"""Independent malformed/wrong-checkout counterexamples for the resolver guard."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import os
import shlex
import shutil
import stat
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.ci import binary_custody
from tools.ci.paired_r5_result import _verify_resolution_files
from tools.ci.tests.p11_test_issuer import cache_old_python_source

MODULE_PATH = Path(__file__).resolve().parents[1] / "paired_cargo_resolution.py"
SPEC = importlib.util.spec_from_file_location("paired_cargo_resolution", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


@pytest.fixture
def resolution(tmp_path):
    quanta = tmp_path / "quanta"
    paired = tmp_path / "paired"
    workspace = paired / "packages/analysis/quanta-v2"
    consumer = "quanta-runtime"
    names = sorted(MODULE.REQUIRED_QUANTA_PACKAGES)
    quanta.mkdir()
    (quanta / "Cargo.toml").write_text(
        "[workspace]\nmembers = " + json.dumps([f"crates/{name}" for name in names]) + "\n"
    )
    packages = []
    nodes = []
    for name in names:
        manifest = quanta / "crates" / name / "Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
        packages.append(
            {
                "id": name,
                "name": name,
                "manifest_path": str(manifest),
                "source": None,
                "version": "0.1.0",
            }
        )
        nodes.append({"id": name, "dependencies": [], "features": []})
    manifest = workspace / "crates" / consumer / "Cargo.toml"
    manifest.parent.mkdir(parents=True)
    manifest.write_text(f'[package]\nname = "{consumer}"\nversion = "0.1.0"\n')
    (workspace / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/quanta-runtime"]\n')
    (workspace / "Cargo.lock").write_text("nested-lock-authority\n")
    (paired / "Cargo.lock").write_text("unrelated-root-lock\n")
    packages.append(
        {
            "id": consumer,
            "name": consumer,
            "manifest_path": str(manifest),
            "source": None,
            "version": "0.1.0",
        }
    )
    nodes.append({"id": consumer, "dependencies": names, "features": ["index-sdk-ingress"]})
    metadata = {
        "version": 1,
        "workspace_root": str(workspace),
        "packages": packages,
        "resolve": {"nodes": nodes},
    }
    return quanta, paired, consumer, metadata


@pytest.mark.parametrize("alias", [False, True])
def test_binary_custody_detects_shared_output_replacement(tmp_path, alias):
    built = tmp_path / "built"
    built.write_bytes(b"#!/bin/sh\nprintf '%s' original\n")
    built.chmod(0o700)
    provided = built if alias else tmp_path / "provided"
    if not alias:
        provided.write_bytes(built.read_bytes())
        provided.chmod(0o700)
    directory = tmp_path / "custody"
    directory.mkdir(mode=0o700)
    pinned = directory / "daemon"
    digest = binary_custody.pin(built, provided, pinned)
    binary_custody.verify(digest, [built, provided, pinned])
    built.write_bytes(b"#!/bin/sh\nprintf '%s' replaced\n")
    # The process still uses the admitted copy, not the shared target output.
    assert subprocess.check_output([str(pinned)]) == b"original"
    with pytest.raises(ValueError, match="changed"):
        binary_custody.verify(digest, [built, provided, pinned])


def test_binary_custody_refuses_symlink_and_nonprivate_destination(tmp_path):
    built = tmp_path / "built"
    built.write_bytes(b"#!/bin/sh\nexit 0\n")
    built.chmod(0o700)
    alias = tmp_path / "alias"
    alias.symlink_to(built)
    directory = tmp_path / "custody"
    directory.mkdir(mode=0o700)
    pinned = directory / "daemon"
    with pytest.raises(OSError):
        binary_custody.pin(built, alias, pinned)
    assert not pinned.exists()
    directory.chmod(0o755)
    with pytest.raises(ValueError, match="private"):
        binary_custody.pin(built, built, pinned)
    assert not pinned.exists()


@pytest.mark.parametrize("failure", ["file-sync", "source-drift"])
def test_binary_custody_retry_requires_no_output_from_failed_pin(tmp_path, monkeypatch, failure):
    built = tmp_path / "built"
    raw = b"#!/bin/sh\nexit 0\n"
    built.write_bytes(raw)
    built.chmod(0o700)
    directory = tmp_path / "custody"
    directory.mkdir(mode=0o700)
    pinned = directory / "daemon"
    with monkeypatch.context() as patch:
        if failure == "file-sync":

            def fail_regular_sync(fd):
                if stat.S_ISREG(os.fstat(fd).st_mode):
                    raise OSError("injected executable sync failure")
                real_fsync(fd)

            real_fsync = os.fsync
            patch.setattr(binary_custody.os, "fsync", fail_regular_sync)
            error = pytest.raises(OSError, match="executable sync failure")
        else:

            def drift_before_final_verification(digest, paths, *, archive=None):
                if pinned in paths:
                    built.write_bytes(b"changed after copy")
                real_verify(digest, paths, archive=archive)

            real_verify = binary_custody.verify
            patch.setattr(binary_custody, "verify", drift_before_final_verification)
            error = pytest.raises(ValueError, match="changed during proof")
        with error:
            binary_custody.pin(built, built, pinned)
    assert list(directory.iterdir()) == []

    built.write_bytes(raw)
    binary_custody.verify(binary_custody.pin(built, built, pinned), [pinned])
    with pytest.raises(FileExistsError):
        binary_custody.pin(built, built, pinned)
    assert pinned.read_bytes() == raw


def test_p10_archive_pin_rejects_changed_bytes_before_copy(tmp_path):
    archive = tmp_path / "archive"
    archive.write_bytes(b"changed archive")
    archive.chmod(0o600)
    directory = tmp_path / "custody"
    directory.mkdir(mode=0o700)
    pinned = directory / "daemon"

    with pytest.raises(ValueError, match="P10 archived daemon differs"):
        binary_custody.pin(
            archive, archive, pinned, archive_sha256=hashlib.sha256(b"approved archive").hexdigest()
        )
    assert not pinned.exists()


def _commit_fixture_checkout(root: Path) -> None:
    for args in (
        ["init", "-q"],
        ["add", "."],
        [
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ):
        subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)


@pytest.fixture
def paired_bootstrap(resolution, tmp_path, monkeypatch):
    quanta, paired, _, _ = resolution
    runner_path = Path("scripts/verify-repomap-cross-repo.sh")
    runner = quanta / runner_path
    runner.parent.mkdir()
    shutil.copy2(Path(__file__).resolve().parents[3] / runner_path, runner)
    helper_path = "scripts/lib/python-env.sh"
    helper = paired / helper_path
    helper.parent.mkdir(parents=True)
    marker = tmp_path / "python-helper-sourced"
    helper.write_text(
        f": > {shlex.quote(str(marker))}\n"
        "cg_resolve_python_bin() {\n"
        "  printf '%s\\n' 'fixture resolver reached' >&2\n"
        "  return 67\n"
        "}\n"
    )
    monkeypatch.setenv("QUANTA_P11_R5_QBC_LANE", "registered-resolution")
    return quanta, paired, runner, helper, marker


@pytest.mark.parametrize("entry", ["just", "direct"])
def test_cross_repo_launch_does_not_run_ambient_shell_startup(
    paired_bootstrap,
    tmp_path,
    monkeypatch,
    entry,
):
    quanta, paired, runner, _, helper_marker = paired_bootstrap
    shutil.copyfile(Path(__file__).resolve().parents[3] / "Justfile", quanta / "Justfile")
    paired = paired.rename(tmp_path / "paired $R5_FIXTURE_VALUE with spaces")
    for root in (quanta, paired):
        _commit_fixture_checkout(root)

    login_marker = tmp_path / "login-started"
    bash_marker = tmp_path / "bash-started"
    (tmp_path / ".zshenv").write_text(f": > {shlex.quote(str(login_marker))}\n")
    bash_env = tmp_path / "bash-env"
    bash_env.write_text(f": > {shlex.quote(str(bash_marker))}\n")
    monkeypatch.setenv("ZDOTDIR", str(tmp_path))
    monkeypatch.setenv("BASH_ENV", str(bash_env))
    monkeypatch.setenv("R5_FIXTURE_VALUE", "expanded")

    command = ["just", "rust-verify-hellgate-cross-repo"] if entry == "just" else [str(runner)]
    completed = subprocess.run(
        [*command, str(paired)],
        cwd=quanta,
        text=True,
        capture_output=True,
        check=False,
    )

    assert (completed.returncode, login_marker.exists(), bash_marker.exists()) == (
        67,
        False,
        False,
    ), completed.stderr
    assert "fixture resolver reached" in completed.stderr.splitlines()
    assert helper_marker.exists()


def test_signed_cross_repo_refuses_bash_without_startup_isolation(
    paired_bootstrap,
    tmp_path,
    monkeypatch,
):
    quanta, paired, runner, _, helper_marker = paired_bootstrap
    for root in (quanta, paired):
        _commit_fixture_checkout(root)
    monkeypatch.delenv("BASH_ENV", raising=False)
    monkeypatch.setenv("QUANTA_P11_SIGNER_SOCKET", str(tmp_path / "fixture-socket"))

    completed = subprocess.run(
        ["/bin/bash", str(runner), str(paired)],
        text=True,
        capture_output=True,
        check=False,
    )

    assert completed.returncode == 1, completed.stderr
    assert completed.stderr.strip() == "P11 signing requires the isolated cross-repo launcher"
    assert not helper_marker.exists()


@pytest.mark.parametrize(
    "source_state",
    [
        "dirty-quanta",
        "dirty-paired",
        "hidden-helper",
        "hidden-quanta",
        "hidden-paired",
        "normalized-helper",
        "foreign-git-env",
        "nested-paired",
        "helper-symlink",
    ],
)
def test_cross_repo_script_refuses_unbound_source_before_python_helper(
    paired_bootstrap,
    monkeypatch,
    source_state,
):
    quanta, paired, runner, helper, marker = paired_bootstrap
    helper_path = helper.relative_to(paired)
    supplied_root = paired
    if source_state == "normalized-helper":
        (paired / ".gitattributes").write_text(f"{helper_path} text eol=crlf\n")
    elif source_state == "nested-paired":
        supplied_root = paired / "nested"
        nested_helper = supplied_root / helper_path
        nested_helper.parent.mkdir(parents=True)
        nested_helper.write_bytes(helper.read_bytes())
    elif source_state == "helper-symlink":
        target = helper.with_name("same-bytes.sh")
        helper.rename(target)
        helper.symlink_to(target.name)

    for root in (quanta, paired):
        _commit_fixture_checkout(root)

    expected_error = "cross-repo proof requires a clean checkout"
    if source_state.startswith("hidden-"):
        dirty_root = quanta if source_state == "hidden-quanta" else paired
        path = {
            "hidden-helper": helper,
            "hidden-quanta": quanta / "Cargo.toml",
            "hidden-paired": paired / "packages/analysis/quanta-v2/Cargo.toml",
        }[source_state]
        flag = "--skip-worktree" if source_state == "hidden-paired" else "--assume-unchanged"
        git = ["git", "-C", str(dirty_root)]
        subprocess.run(
            [*git, "update-index", flag, "--", str(path.relative_to(dirty_root))],
            check=True,
            capture_output=True,
        )
        path.write_bytes(path.read_bytes() + b"# hidden worktree change\n")
        assert subprocess.check_output([*git, "status", "--porcelain"]) == b""
        expected_error = "cross-repo proof refuses hidden Git index entries"
    elif source_state == "normalized-helper":
        helper.write_bytes(helper.read_bytes().replace(b"\n", b"\r\n"))
        git = ["git", "-C", str(paired)]
        subprocess.run([*git, "add", "--", str(helper_path)], check=True, capture_output=True)
        assert subprocess.check_output([*git, "status", "--porcelain"]) == b""
        assert subprocess.check_output([*git, "show", f"HEAD:{helper_path}"]) != helper.read_bytes()
        expected_error = "paired Python helper differs from the frozen Git blob"
    elif source_state == "nested-paired":
        expected_error = "cross-repo proof requires the checkout root"
    elif source_state == "helper-symlink":
        expected_error = "paired Python helper is not a physical regular file"
    else:
        dirty_root = paired if source_state == "dirty-paired" else quanta
        (dirty_root / "dirty.txt").touch()
        if source_state == "foreign-git-env":
            monkeypatch.setenv("GIT_DIR", str(paired / ".git"))
            monkeypatch.setenv("GIT_WORK_TREE", str(paired))

    completed = subprocess.run(
        [str(runner), str(supplied_root)],
        text=True,
        capture_output=True,
    )

    assert not marker.exists()
    assert completed.returncode == 1
    assert expected_error in completed.stderr


def test_cross_repo_script_runs_checked_helper_bytes_after_comparison(
    paired_bootstrap,
    tmp_path,
    monkeypatch,
):
    quanta, paired, runner, _, marker = paired_bootstrap
    for root in (quanta, paired):
        _commit_fixture_checkout(root)
    real_cmp = shutil.which("cmp")
    assert real_cmp is not None
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    comparator = bin_dir / "cmp"
    # Interleave a write immediately after a successful comparison if the
    # candidate bytes are still backed by a writable staging file.
    comparator.write_text(
        "#!/usr/bin/env bash\n"
        f'{shlex.quote(real_cmp)} "$@" || exit $?\n'
        "for last; do :; done\n"
        'if [[ -f "$last" ]]; then\n'
        """  printf '\\n%s\\n' 'exit 66' >> "$last"\n"""
        "fi\n"
    )
    comparator.chmod(0o700)
    monkeypatch.setenv("TMPDIR", str(tmp_path))
    monkeypatch.setenv("PATH", str(bin_dir) + os.pathsep + os.environ["PATH"])

    completed = subprocess.run(
        [str(runner), str(paired)],
        text=True,
        capture_output=True,
    )

    assert completed.returncode == 67
    assert completed.stderr.strip() == "fixture resolver reached"
    assert marker.exists()


@pytest.mark.parametrize("mutate", [False, True])
@pytest.mark.parametrize("temporary", ["default", "custom", "alias"])
@pytest.mark.parametrize("preflight_failure", [False, True])
@pytest.mark.parametrize(
    ("use_p10", "use_recorded"), [(False, False), (True, False), (False, True)]
)
def test_cross_repo_script_consumes_pinned_binary_and_rejects_alias_drift(
    resolution,
    tmp_path,
    monkeypatch,
    mutate,
    temporary,
    preflight_failure,
    use_p10,
    use_recorded,
):
    import socket

    quanta, paired, _, _ = resolution
    repo_root = Path(__file__).resolve().parents[3]
    monkeypatch.chdir(tmp_path)
    with socket.socket(socket.AF_UNIX) as signer:
        signer.bind("signer")
    (tmp_path / "signer-alias").symlink_to(tmp_path / "signer")
    monkeypatch.setenv("QUANTA_P11_SIGNER_SOCKET", str(tmp_path / "signer"))
    monkeypatch.setenv("SSH_AUTH_SOCK", str(tmp_path / "signer-alias"))
    startup = tmp_path / "ambient-python"
    startup.mkdir()
    observed_signer = tmp_path / "signer-seen-during-startup"
    observed_python = tmp_path / "ambient-python-ran"
    (startup / "sitecustomize.py").write_text(
        "import os\n"
        "from pathlib import Path\n"
        f"Path({str(observed_python)!r}).touch()\n"
        "if os.environ.get('QUANTA_P11_SIGNER_SOCKET') or os.environ.get('SSH_AUTH_SOCK'):\n"
        f"    Path({str(observed_signer)!r}).touch()\n"
    )
    python_path = os.environ.get("PYTHONPATH")
    monkeypatch.setenv(
        "PYTHONPATH",
        os.pathsep.join(part for part in (str(startup), python_path) if part),
    )
    python_env = paired / "scripts/lib/python-env.sh"
    python_env.parent.mkdir(parents=True)
    python_env.write_text(
        "cg_resolve_python_bin() {\n"
        '  test -z "${SSH_AUTH_SOCK:-}${QUANTA_P11_SIGNER_SOCKET:-}" || exit 86\n'
        '  test -z "${paired_signer_socket:-}" || exit 86\n'
        "  printf '%s\\n' " + shlex.quote(sys.executable) + "\n}\n"
    )
    monkeypatch.delenv("QUANTA_P11_R5_EVIDENCE_ROOT", raising=False)
    monkeypatch.delenv("QUANTA_RETIREMENT_DAEMON_BUILD", raising=False)
    monkeypatch.delenv("QUANTA_RETIREMENT_DAEMON_RECEIPT_SHA256", raising=False)
    if temporary == "default":
        monkeypatch.delenv("TMPDIR", raising=False)
        temporary_root = Path("/tmp").resolve()
    else:
        temporary_root = tmp_path / "temporary root"
        temporary_root.mkdir()
        named_root = temporary_root
        if temporary == "alias":
            named_root = tmp_path / "temporary alias"
            named_root.symlink_to(temporary_root, target_is_directory=True)
        monkeypatch.setenv("TMPDIR", str(named_root))
        temporary_root = temporary_root.resolve()
    for relative in (
        "scripts/verify-repomap-cross-repo.sh",
        "tools/ci/paired_cargo_resolution.py",
        "tools/ci/binary_custody.py",
        "tools/ci/daemon_execution_evidence.py",
        "tools/ci/lint/handoff_validation.py",
        "tools/ci/proof-authority.toml",
        "tools/ci/proof_json.py",
    ):
        target = quanta / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(repo_root / relative, target)
    # The fixture rewrites copied inputs even when the issuing source is sealed.
    (quanta / "scripts/verify-repomap-cross-repo.sh").chmod(0o700)
    if not use_p10:
        registry = quanta / "tools/ci/proof-authority.toml"
        registry.write_text(
            registry.read_text().replace(
                'features = "index-sdk-ingress-surface"',
                'features = "index-sdk-ingress-surface,fixture-replay-owner"',
            )
        )
    # This scenario owns binary custody; resolver/selected-run behavior has
    # independent tests and is stubbed so no QBC lane or Rust build is needed.
    (quanta / "tools/ci/paired_cargo_resolution.py").write_text(
        "import argparse, json\n"
        "p=argparse.ArgumentParser()\n"
        "p.add_argument('--consumer'); p.add_argument('--quanta-root'); "
        "p.add_argument('--paired-root'); p.add_argument('--feature'); p.add_argument('--qbc-lane')\n"
        "a=p.parse_args(); print(json.dumps({'consumer':a.consumer,'feature':a.feature}))\n"
    )
    target_dir = tmp_path / "target with spaces"
    (target_dir / "release").mkdir(parents=True)
    built = target_dir / "release/quanta-index-searchd"
    built.write_bytes(b"#!/bin/sh\nprintf original\n")
    built.chmod(0o700)
    select_marker = tmp_path / "select-called.txt"
    if use_recorded:
        monkeypatch.setenv("QUANTA_RETIREMENT_DAEMON_BUILD", str(built.parent))
        monkeypatch.setenv("QUANTA_RETIREMENT_DAEMON_RECEIPT_SHA256", "d" * 64)
        (quanta / "tools/ci/check-tantivy-retirement.py").write_text(
            "import pathlib, sys\n"
            "assert sys.argv[1:] == [\n"
            "    '--select-captured-daemon', '--release-build', " + repr(str(built.parent)) + ",\n"
            "    '--expected-build-receipt-sha256', '" + "d" * 64 + "',\n"
            "]\n"
            f"pathlib.Path({str(select_marker)!r}).touch()\n"
            f"print({hashlib.sha256(built.read_bytes()).hexdigest()!r})\n"
            f"print({str(built)!r})\n"
        )
    archive = (
        quanta
        / "artifacts/proof-authority/binaries"
        / hashlib.sha256(built.read_bytes()).hexdigest()
    )
    if use_p10:
        archive.parent.mkdir(parents=True)
        archive.write_bytes(built.read_bytes())
        archive.chmod(0o600)
    archive_path = archive.relative_to(quanta).as_posix()
    evidence_parent_record = tmp_path / "evidence-parent.txt"
    (quanta / "tools/ci/paired_r5_result.py").write_text(
        "import hashlib, json, pathlib, subprocess, sys\n"
        f"sys.path.insert(0, {str(repo_root)!r})\n"
        "from tools.ci import paired_r5_result as runner, paired_r5_release as release\n"
        "from tools.ci.daemon_execution_evidence import R5_PROOF, live_proof\n"
        "def local_phase(pair, root, evidence, pair_head, head, case, completed, completion, "
        "owner, daemon_digest, binary_paths, lane, *, archive, cases):\n"
        f"    assert archive == (root / {archive_path!r} if {use_p10!r} else None)\n"
        "    assert case in cases\n"
        "    pinned = binary_paths['custody']\n"
        "    assert hashlib.sha256(pinned.read_bytes()).hexdigest() == daemon_digest\n"
        f"    assert pinned != pathlib.Path({str(built)!r})\n"
        f"    if {use_p10!r}: assert binary_paths['built'] == root / {archive_path!r}\n"
        "    assert subprocess.check_output([str(pinned)]) == b'original'\n"
        f"    assert evidence.parent.parent == pathlib.Path({str(temporary_root)!r})\n"
        "    assert evidence == evidence.resolve() and evidence.is_dir()\n"
        f"    pathlib.Path({str(evidence_parent_record)!r}).write_text(str(evidence.parent))\n"
        f"    if {(mutate and not use_p10)!r}: pathlib.Path({str(built)!r}).write_bytes(b'changed release bytes')\n"
        "    return {}\n"
        "def qbc_modules(_, _head):\n"
        f"    if {preflight_failure!r}: raise SystemExit(7)\n"
        "    return None, None, None\n"
        "runner._qbc_modules = qbc_modules\n"
        "runner._one = local_phase\n"
        "runner.verify_runner_capture = lambda *args, **kwargs: None\n"
        "runner._verify_resolution_files = lambda *args, **kwargs: None\n"
        "runner._resolved_after = lambda root, pair, consumer, feature, lane: dict(consumer=consumer, feature=feature)\n"
        "def admit_release(root, pair, daemon, **options):\n"
        f"    if not {use_p10!r}: return None\n"
        f"    assert daemon is None or daemon == {archive.name!r}\n"
        "    import jsonschema\n"
        "    assert callable(jsonschema.validate)\n"
        "    if daemon is not None:\n"
        f"        assert options['signer_socket'] == {str(tmp_path / 'signer')!r}\n"
        f"    return {{'binary_path': {archive_path!r}, 'proof': live_proof(root, R5_PROOF)}}\n"
        "release.admit_release = admit_release\n"
        "release.prepare_terminal = lambda *args, **kwargs: pathlib.Path('fixture-terminal')\n"
        "raise SystemExit(runner.main(standalone=True))\n"
    )
    build_marker = tmp_path / "build-called.txt"
    zsh_started = tmp_path / "ambient-zsh-ran"
    (tmp_path / ".zshenv").write_text(f": > {shlex.quote(str(zsh_started))}\n")
    monkeypatch.setenv("ZDOTDIR", str(tmp_path))
    (quanta / "Justfile").write_text(
        'set shell := ["/bin/zsh", "-lc"]\n'
        "rust-build-release-native-daemon:\n"
        f"    @touch {shlex.quote(str(build_marker))}\n"
        f'    @printf "%s\\n" {shlex.quote("Verified Native daemon " + str(built))}\n'
        f"    @exit {79 if use_p10 else 0}\n"
    )
    for root in (quanta, paired):
        (root / ".gitignore").write_text("__pycache__/\n", encoding="utf-8")
        _commit_fixture_checkout(root)
    cache_old_python_source(
        quanta / "tools/ci/daemon_execution_evidence.py",
        b"raise RuntimeError('stale registry module')\n",
    )
    if use_p10 and mutate:
        built.write_bytes(b"#!/bin/sh\nprintf different\n")
    if use_p10 and not mutate and temporary == "default":
        monkeypatch.delenv("QUANTA_INDEX_SEARCHD_BIN", raising=False)
    else:
        monkeypatch.setenv("QUANTA_INDEX_SEARCHD_BIN", str(built))
    monkeypatch.setenv("QUANTA_P11_R5_QBC_LANE", "registered-resolution")
    completed = subprocess.run(
        [str(quanta / "scripts/verify-repomap-cross-repo.sh"), str(paired)],
        text=True,
        capture_output=True,
    )
    if evidence_parent_record.exists():
        # Remove only this invocation's fresh candidate and its two directories.
        evidence_parent = Path(evidence_parent_record.read_text())
        assert evidence_parent.parent == temporary_root
        candidate = evidence_parent / "result" / "paired-r5-result.json"
        assert candidate.exists() == (not mutate)
        candidate.unlink(missing_ok=True)
        candidate.parent.rmdir()
        evidence_parent.rmdir()
    if preflight_failure or mutate:
        assert "paired-daemon-sha256:" not in completed.stdout
    if preflight_failure:
        assert completed.returncode == 7
        assert not build_marker.exists()
        assert not evidence_parent_record.exists()
    elif mutate:
        assert completed.returncode != 0
        assert "release daemon bytes changed" in completed.stderr
        assert evidence_parent_record.exists() == (not use_p10)
    else:
        assert completed.returncode == 0, completed.stderr
        assert "paired-daemon-sha256:" in completed.stdout
        assert ("paired-r5-release-terminal:" in completed.stdout) == use_p10
    assert select_marker.exists() == (use_recorded and not preflight_failure)
    assert build_marker.exists() == (not use_p10 and not use_recorded and not preflight_failure)
    assert not observed_signer.exists()
    assert [marker.name for marker in (observed_python, zsh_started) if marker.exists()] == []


def validate(resolution):
    quanta, paired, consumer, metadata = resolution
    return MODULE.validate_resolution(
        metadata, quanta_root=quanta, paired_root=paired, consumer=consumer
    )


def test_exact_root_binds_nested_resolver_lock_and_no_local_paths(resolution):
    result = validate(resolution)
    assert result["dependency_lock"]["path"] == "packages/analysis/quanta-v2/Cargo.lock"
    assert result["dependency_lock"]["sha256"] == MODULE._digest(
        resolution[1] / result["dependency_lock"]["path"]
    )
    assert set(item["name"] for item in result["packages"]) == MODULE.REQUIRED_QUANTA_PACKAGES
    assert str(resolution[0].parent) not in json.dumps(result)


def test_actual_resolver_output_roundtrips_into_paired_runner(resolution):
    quanta, paired, consumer, metadata = resolution
    metadata["resolve"]["nodes"][-1]["features"].append("retrieval-authority-contract-surface")
    runtime = validate(resolution)
    kernel = "quanta-runtime-retrieval-kernel"
    kernel_manifest = paired / "packages/analysis/quanta-v2/crates" / kernel / "Cargo.toml"
    kernel_manifest.parent.mkdir()
    kernel_manifest.write_text(f'[package]\nname = "{kernel}"\nversion = "0.1.0"\n')
    kernel_metadata = copy.deepcopy(metadata)
    kernel_metadata["packages"][-1].update(
        id=kernel, name=kernel, manifest_path=str(kernel_manifest)
    )
    kernel_metadata["resolve"]["nodes"][-1].update(
        id=kernel, features=["index-sdk-ingress-surface"]
    )
    kernel_result = MODULE.validate_resolution(
        kernel_metadata, quanta_root=quanta, paired_root=paired, consumer=kernel
    )
    assert (
        _verify_resolution_files(quanta, paired, {"runtime": runtime, "kernel": kernel_result})
        is None
    )


def test_wrong_checkout_same_name_same_bytes_is_refused(resolution, tmp_path):
    quanta, _, _, metadata = resolution
    package = metadata["packages"][0]
    original = Path(package["manifest_path"])
    sidecar = tmp_path / "quanta-sidecar" / original.relative_to(quanta)
    sidecar.parent.mkdir(parents=True)
    sidecar.write_bytes(original.read_bytes())
    package["manifest_path"] = str(sidecar)
    with pytest.raises(ValueError, match="another source"):
        validate(resolution)


def test_manifest_symlink_to_other_checkout_is_refused(resolution, tmp_path):
    package = resolution[3]["packages"][0]
    manifest = Path(package["manifest_path"])
    sidecar = tmp_path / "sidecar-Cargo.toml"
    sidecar.write_bytes(manifest.read_bytes())
    manifest.unlink()
    manifest.symlink_to(sidecar)
    with pytest.raises(ValueError, match="escapes"):
        validate(resolution)


def test_canonical_symlink_alias_to_expected_checkout_is_accepted(resolution):
    quanta, paired, consumer, metadata = resolution
    alias = quanta.parent / "quanta-alias"
    alias.symlink_to(quanta, target_is_directory=True)
    metadata["packages"][0]["manifest_path"] = str(
        alias / Path(metadata["packages"][0]["manifest_path"]).relative_to(quanta)
    )
    assert MODULE.validate_resolution(
        metadata, quanta_root=quanta, paired_root=paired, consumer=consumer
    )["packages"]


@pytest.mark.parametrize(
    "failure",
    [
        None,
        "stale",
        "output-change",
        "nonzero",
        "wrong-command",
        "bad-json",
        "unregistered",
        "effective-failure",
        "raw-unobserved",
        "raw-failure",
        "missing-output",
        "owner-change",
        "wrong-path",
        "wrong-nonce",
        "wrong-cwd",
    ],
)
def test_qbc_resolver_uses_own_stable_auxiliary_stdout_and_refuses_bad_owners(
    resolution, tmp_path, monkeypatch, failure
):
    quanta, paired, consumer, metadata = resolution
    state = tmp_path / "state"
    state.mkdir()
    lane = "registered-resolution"
    nonce = ""
    status_calls = 0
    command = [
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--no-default-features",
        "--manifest-path",
        f"packages/analysis/quanta-v2/crates/{consumer}/Cargo.toml",
        "--features",
        "index-sdk-ingress",
    ]

    def fake_status(source, requested_lane, environment):
        nonlocal status_calls
        status_calls += 1
        assert source == paired and requested_lane == lane
        if failure == "unregistered":
            raise ValueError(f"QBC lane registration missing: {lane}")
        current = status_calls > 1
        run_id = "old" if not current or failure == "stale" else "own"
        if failure == "owner-change" and status_calls == 3:
            run_id = "other"
        output_path = (
            state
            / "execution-roots/owner-key/lanes/registered-resolution/last-completed-run/stdout.log"
        )
        item = {
            "lane": lane,
            "lane_key": lane,
            "execution_root": str(paired),
            "registered": True,
            "receipt": {
                "last_run_id_v1": run_id,
                "run_state": "finished",
                "command": command if failure != "wrong-command" else command[:-1],
                "command_cwd_v1": str(quanta if failure == "wrong-cwd" else paired),
                "last_exit_code": 125 if failure == "effective-failure" else 0,
                "command_exit_code_v1": (
                    None if failure == "raw-unobserved" else 1 if failure == "raw-failure" else 0
                ),
                "last_stdout_path_v1": str(quanta if failure == "wrong-path" else output_path),
            },
            "meta": {
                "user_meta": {
                    "paired_r5_resolution_nonce": "other" if failure == "wrong-nonce" else nonce
                },
                "command_context": {"last_run_id_v1": run_id},
            },
        }
        return ({"state_root": str(state), "execution_root_key": "owner-key"}, item)

    def fake_run(argv, **_kwargs):
        nonlocal nonce
        assert argv[:4] == [str(paired / "scripts/quanta-build-cli"), "cargo", "--lane", lane]
        nonce = argv[5].split("=", 1)[1]
        assert argv[6:] == ["--", *command[1:]]
        return SimpleNamespace(returncode=1 if failure == "nonzero" else 0, stdout=b"", stderr=b"")

    raw = b"{" if failure == "bad-json" else json.dumps(metadata).encode()
    reads = 0

    def fake_read(_path):
        nonlocal reads
        reads += 1
        if failure == "missing-output":
            raise FileNotFoundError("metadata output absent")
        if failure == "output-change" and reads == 2:
            return b"changed", (1, 2, 3, 4, 5)
        return raw, (1, 2, len(raw), 4, 5)

    monkeypatch.setattr(MODULE, "frozen_source_head", lambda _root: "f" * 40)
    monkeypatch.setattr(MODULE, "_status", fake_status)
    monkeypatch.setattr(MODULE, "_read_bounded_regular", fake_read)
    monkeypatch.setattr(MODULE.subprocess, "run", fake_run)

    def invoke():
        return MODULE.resolve_from_qbc(
            quanta_root=quanta,
            paired_root=paired,
            consumer=consumer,
            feature="index-sdk-ingress",
            lane=lane,
        )

    if failure in (None, "raw-unobserved"):
        assert invoke()["consumer"] == consumer
        assert reads == 2 and status_calls == 3
    else:
        with pytest.raises(ValueError):
            invoke()
        if failure == "unregistered":
            assert status_calls == 1
        if failure == "bad-json":
            assert reads == 1


@pytest.mark.parametrize("source_state", ["hidden", "foreign-git-environment"])
def test_qbc_resolver_checks_source_before_invoking_external_cli(
    resolution,
    tmp_path,
    monkeypatch,
    source_state,
):
    quanta, paired, consumer, _ = resolution
    marker = tmp_path / "qbc-cli-invoked"
    cli = paired / "scripts/quanta-build-cli"
    cli.parent.mkdir()
    cli.write_text(f"#!/bin/sh\n: > {shlex.quote(str(marker))}\nexit 91\n")
    cli.chmod(0o700)
    for root in (quanta, paired):
        _commit_fixture_checkout(root)
    if source_state == "hidden":
        subprocess.run(
            [
                "git",
                "-C",
                str(paired),
                "update-index",
                "--skip-worktree",
                "scripts/quanta-build-cli",
            ],
            check=True,
            capture_output=True,
        )
        cli.write_bytes(cli.read_bytes() + b"# changed worktree\n")
        assert subprocess.check_output(["git", "-C", str(paired), "status", "--porcelain"]) == b""
        expected_error = "hidden Git index entries"
    else:
        (quanta / "dirty.txt").touch()
        monkeypatch.setenv("GIT_DIR", str(paired / ".git"))
        monkeypatch.setenv("GIT_WORK_TREE", str(paired))
        expected_error = "Git repository environment"

    with pytest.raises(ValueError) as error:
        MODULE.resolve_from_qbc(
            quanta_root=quanta,
            paired_root=paired,
            consumer=consumer,
            feature="index-sdk-ingress",
            lane="registered-resolution",
        )
    assert not marker.exists()
    assert expected_error in str(error.value)


def test_qbc_resolver_keeps_the_dedicated_signer_out_of_external_cli(
    resolution,
    tmp_path,
    monkeypatch,
):
    import socket

    quanta, paired, consumer, _ = resolution
    monkeypatch.chdir(tmp_path)
    with socket.socket(socket.AF_UNIX) as signer:
        signer.bind("signer")
    signer_alias = tmp_path / "signer-alias"
    signer_alias.symlink_to("signer")
    monkeypatch.setenv("QUANTA_P11_SIGNER_SOCKET", str(tmp_path / "signer"))
    monkeypatch.setenv("SSH_AUTH_SOCK", str(signer_alias))
    cli = paired / "scripts/quanta-build-cli"
    cli.parent.mkdir()
    cli.write_text(
        '#!/bin/sh\ntest -z "${QUANTA_P11_SIGNER_SOCKET:-}${SSH_AUTH_SOCK:-}" || exit 86\nexit 91\n'
    )
    cli.chmod(0o700)
    for root in (quanta, paired):
        _commit_fixture_checkout(root)

    with pytest.raises(ValueError, match="QBC observational status failed: exit 91"):
        MODULE.resolve_from_qbc(
            quanta_root=quanta,
            paired_root=paired,
            consumer=consumer,
            feature="index-sdk-ingress",
            lane="registered-resolution",
        )
    assert os.environ["SSH_AUTH_SOCK"] == str(signer_alias)
    assert os.environ["QUANTA_P11_SIGNER_SOCKET"] == str(tmp_path / "signer")


def test_qbc_resolver_executes_source_with_an_older_python_cache(resolution, tmp_path, monkeypatch):
    quanta, paired, consumer, _ = resolution
    cli = paired / "scripts/quanta-build-cli"
    cli.parent.mkdir()
    driver = paired / "qbc_driver.py"
    driver.write_text("from status_profile import STATUS\nraise SystemExit(STATUS)\n")
    cli.write_text(
        f"#!/usr/bin/env bash\nexec {shlex.quote(sys.executable)} {shlex.quote(str(driver))}\n"
    )
    cli.chmod(0o700)
    profile = paired / "status_profile.py"
    profile.write_text("STATUS = 91\n")
    (paired / ".gitignore").write_text("__pycache__/\n")
    for root in (quanta, paired):
        _commit_fixture_checkout(root)
    cache_old_python_source(profile, b"STATUS = 86\n")
    monkeypatch.delenv("PYTHONPYCACHEPREFIX", raising=False)
    bash_started = tmp_path / "external-bash-ran"
    bash_env = tmp_path / "bash-env"
    bash_env.write_text(f": > {shlex.quote(str(bash_started))}\n")
    python_started = tmp_path / "external-python-ran"
    (tmp_path / "sitecustomize.py").write_text(
        f"from pathlib import Path\nPath({str(python_started)!r}).touch()\n"
    )
    monkeypatch.setenv("BASH_ENV", str(bash_env))
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))

    with pytest.raises(ValueError, match="QBC observational status failed: exit 91"):
        MODULE.resolve_from_qbc(
            quanta_root=quanta,
            paired_root=paired,
            consumer=consumer,
            feature="index-sdk-ingress",
            lane="registered-resolution",
        )
    assert [marker.name for marker in (bash_started, python_started) if marker.exists()] == []
    assert os.environ["BASH_ENV"] == str(bash_env)
    assert os.environ["PYTHONPATH"] == str(tmp_path)


def test_qbc_auxiliary_output_reader_refuses_fifo_without_waiting_for_writer(tmp_path):
    output = tmp_path / "stdout.log"
    os.mkfifo(output)
    result = subprocess.run(
        [
            sys.executable,
            "-B",
            "-c",
            "import runpy, sys\n"
            "from pathlib import Path\n"
            "reader = runpy.run_path(sys.argv[1])['_read_bounded_regular']\n"
            "try:\n"
            "    reader(Path(sys.argv[2]))\n"
            "except ValueError as error:\n"
            "    sys.exit(str(error))\n",
            str(MODULE_PATH),
            str(output),
        ],
        text=True,
        capture_output=True,
        timeout=4,
        check=False,
    )
    assert result.returncode == 1
    assert result.stderr.strip() == "QBC metadata output is not a bounded regular file"


def test_qbc_auxiliary_output_reader_requires_bounded_regular_single_link(tmp_path, monkeypatch):
    output = tmp_path / "stdout.log"
    output.write_bytes(b'{"version":1}')
    assert MODULE._read_bounded_regular(output)[0] == output.read_bytes()
    alias = tmp_path / "alias.log"
    alias.symlink_to(output)
    with pytest.raises(ValueError, match="alias"):
        MODULE._read_bounded_regular(alias)
    hardlink = tmp_path / "hardlink.log"
    os.link(output, hardlink)
    with pytest.raises(ValueError, match="regular"):
        MODULE._read_bounded_regular(output)
    hardlink.unlink()
    monkeypatch.setattr(MODULE, "QBC_METADATA_MAX_BYTES", 4)
    with pytest.raises(ValueError, match="bounded"):
        MODULE._read_bounded_regular(output)


@pytest.mark.parametrize(
    "source", ["registry+https://example.invalid/index", "git+https://example.invalid/quanta"]
)
def test_non_path_dependency_is_refused_even_with_expected_manifest(resolution, source):
    resolution[3]["packages"][0]["source"] = source
    with pytest.raises(ValueError, match="another source"):
        validate(resolution)


def test_missing_source_is_not_assumed_to_be_local(resolution):
    del resolution[3]["packages"][0]["source"]
    with pytest.raises(ValueError, match="another source"):
        validate(resolution)


def test_kernel_feature_profile_is_admitted_independently(resolution):
    quanta, paired, consumer, metadata = resolution
    kernel = "quanta-runtime-retrieval-kernel"
    manifest = paired / "packages/analysis/quanta-v2/crates" / kernel / "Cargo.toml"
    manifest.parent.mkdir()
    manifest.write_text(f'[package]\nname = "{kernel}"\nversion = "0.1.0"\n')
    metadata["packages"][-1].update(id=kernel, name=kernel, manifest_path=str(manifest))
    metadata["resolve"]["nodes"][-1].update(id=kernel, features=["index-sdk-ingress-surface"])
    result = MODULE.validate_resolution(
        metadata, quanta_root=quanta, paired_root=paired, consumer=kernel
    )
    assert result["consumer"] == kernel
    assert result["consumer_features"] == ["index-sdk-ingress-surface"]


def test_required_package_not_reachable_is_refused(resolution):
    resolution[3]["resolve"]["nodes"][-1]["dependencies"].pop()
    with pytest.raises(ValueError, match="all required"):
        validate(resolution)


@pytest.mark.parametrize(
    "mutation,error",
    [
        ("no_graph", "resolve graph"),
        ("duplicate_package", "duplicate Cargo package"),
        ("duplicate_node", "duplicate Cargo resolve node"),
        ("unknown_edge", "dependency edge"),
        ("missing_node", "lacks a resolve node"),
        ("disabled_feature", "not enabled"),
    ],
)
def test_malformed_or_incomplete_graph_is_refused(resolution, mutation, error):
    metadata = resolution[3]
    if mutation == "no_graph":
        metadata["resolve"] = None
    elif mutation == "duplicate_package":
        metadata["packages"].append(copy.deepcopy(metadata["packages"][0]))
    elif mutation == "duplicate_node":
        metadata["resolve"]["nodes"].append(copy.deepcopy(metadata["resolve"]["nodes"][0]))
    elif mutation == "unknown_edge":
        metadata["resolve"]["nodes"][-1]["dependencies"].append("unknown")
    elif mutation == "missing_node":
        metadata["resolve"]["nodes"].pop(0)
    elif mutation == "disabled_feature":
        metadata["resolve"]["nodes"][-1]["features"] = []
    with pytest.raises(ValueError, match=error):
        validate(resolution)


def test_lock_symlink_outside_workspace_is_refused(resolution):
    lock = resolution[1] / "packages/analysis/quanta-v2/Cargo.lock"
    lock.unlink()
    lock.symlink_to(resolution[1] / "Cargo.lock")
    with pytest.raises(ValueError, match="lock escapes"):
        validate(resolution)


def test_workspace_symlink_escape_is_refused(resolution, tmp_path):
    quanta, paired, _, metadata = resolution
    workspace = paired / "packages/analysis/quanta-v2"
    external = tmp_path / "external-workspace"
    workspace.rename(external)
    workspace.symlink_to(external, target_is_directory=True)
    with pytest.raises(ValueError, match="workspace escapes"):
        validate(resolution)


def test_consumer_manifest_symlink_escape_is_refused(resolution, tmp_path):
    manifest = Path(resolution[3]["packages"][-1]["manifest_path"])
    external = tmp_path / "external-consumer.toml"
    external.write_bytes(manifest.read_bytes())
    manifest.unlink()
    manifest.symlink_to(external)
    with pytest.raises(ValueError, match="consumer escapes"):
        validate(resolution)


def test_duplicate_json_keys_cli_fails_closed(resolution):
    quanta, paired, consumer, _ = resolution
    result = subprocess.run(
        [
            sys.executable,
            str(MODULE_PATH),
            "--quanta-root",
            str(quanta),
            "--paired-root",
            str(paired),
            "--consumer",
            consumer,
        ],
        input='{"version":1,"version":1}',
        text=True,
        capture_output=True,
    )
    assert result.returncode == 1
    assert result.stdout == ""
    assert "duplicate JSON key" in result.stderr


def test_cli_emits_same_validated_mapping(resolution):
    quanta, paired, consumer, metadata = resolution
    result = subprocess.run(
        [
            sys.executable,
            str(MODULE_PATH),
            "--quanta-root",
            str(quanta),
            "--paired-root",
            str(paired),
            "--consumer",
            consumer,
        ],
        input=json.dumps(metadata),
        text=True,
        capture_output=True,
        check=True,
    )
    assert json.loads(result.stdout) == validate(resolution)
