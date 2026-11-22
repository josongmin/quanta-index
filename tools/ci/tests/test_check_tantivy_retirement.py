from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import struct
import subprocess
from pathlib import Path

import pytest

TANTIVY_PACKAGE_ID = "registry+https://github.com/rust-lang/crates.io-index#tantivy-fst@0.5.0"


def release_elf(payload=b"", *, machine=62):
    image = bytearray(64 + 56)
    image[:8] = b"\x7fELF\x02\x01\x01\x03"
    struct.pack_into("<HHIQQ", image, 16, 3, machine, 1, 0x1000, 64)
    struct.pack_into("<HHH", image, 52, 64, 56, 1)
    return bytes(image) + payload


def load_checker():
    path = Path(__file__).resolve().parents[1] / "check-tantivy-retirement.py"
    spec = importlib.util.spec_from_file_location("check_tantivy_retirement", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def checkout(tmp_path, monkeypatch):
    checker = load_checker()
    daemon = "quanta-index-searchd-runtime"
    (tmp_path / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
    (tmp_path / "Cargo.lock").write_text(f'[[package]]\nname = "{daemon}"\n')
    cargo = {
        "tree": f"{daemon} v0.1.0\n",
        "metadata": json.dumps({
            "workspace_members": ["daemon-package"],
            "packages": [{"id": "daemon-package", "name": daemon, "features": {}}],
        }),
    }

    def cargo_output(root, command, *args):
        assert root == tmp_path
        if command == "--lane":
            assert args == (
                checker.NATIVE_RELEASE_LANE, "metadata", "--format-version", "1",
                "--no-deps", "--no-default-features",
            )
            return cargo["metadata"]
        assert "--all-features" in args
        if command == "tree":
            assert "--workspace" in args
            assert args[args.index("--edges") + 1] == "normal,build,dev"
            assert args[args.index("--target") + 1] == "all"
        return cargo[command]

    monkeypatch.setattr(checker, "ROOT", tmp_path)
    monkeypatch.setattr(checker, "cargo_output", cargo_output)
    return checker, cargo


def test_keeps_tantivy_free_lexical_package(checkout):
    checker, cargo = checkout
    cargo["tree"] += "quanta-index-lexical v0.1.0\n"
    assert checker.main() == 0


def test_default_release_daemon_uses_the_native_artifact_producer():
    checker = load_checker()
    command = checker.subprocess.run(
        ["just", "--dry-run", "rust-build-release-daemon"],
        cwd=checker.ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stderr
    assert "--no-default-features" in command
    assert "tools/ci/check-tantivy-retirement.py --build-native-daemon" in command
    assert "--all-features" not in command


@pytest.mark.parametrize(("role", "fault"), [
    ("daemon", None), ("cli", None), ("daemon", "source-drift"), ("daemon", "source-restored"),
    ("cli", "compiler-exit"), ("cli", "foreign-artifact"), ("cli", "wrong-target"),
])
def test_release_build_receipt_round_trips_to_real_shipment_reader(tmp_path, monkeypatch, role, fault):
    from tools.ci.source_closure import ClosureError

    checker = load_checker()
    monkeypatch.setenv("CI", "true")
    monkeypatch.setenv("QUANTA_INDEX_RESOURCE_ADMISSION", "0")
    root = tmp_path / "source"
    root.mkdir()
    source = root / "Cargo.toml"
    source.write_text("[workspace]\n")
    real_run = subprocess.run
    for args in (["init", "-q"], ["add", "Cargo.toml"],
                 ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "-qm", "input"]):
        real_run(["git", *args], cwd=root, check=True)
    target = tmp_path / "cargo-target"
    evidence = tmp_path / "release-evidence"
    inputs = {
        "daemon": (("quanta-index-searchd-runtime", "quanta-index-searchd", "bin", "quanta-index-searchd"),),
        "cli": (
            ("quanta-index-sdk", "quanta_index_sdk", "lib", "libquanta_index_sdk-abc.rlib"),
            ("quanta-index-searchctl", "quanta-index-searchctl", "bin", "quanta-index-searchctl"),
        ),
    }

    def metadata(checkout, *args):
        assert checkout == root
        assert args[:3] == (
            "--lane", "release-native-daemon-bin-lane" if role == "daemon" else "release-bin-lane", "metadata",
        )
        return json.dumps({"target_directory": str(target), "build_directory": str(tmp_path / "cargo-build")})

    def run(command, *args, **kwargs):
        if command[0] != str(root / "scripts/cargow"):
            return real_run(command, *args, **kwargs)
        assert kwargs["cwd"] == root
        assert kwargs.get("env", {}).get("QUANTA_INDEX_RESOURCE_ADMISSION") == "1"
        assert kwargs["env"]["CI"] == "true"
        assert {"--release", "--locked", "--offline", "--message-format=json-render-diagnostics"} <= set(command)
        assert ("--no-default-features" if role == "daemon" else "--all-features") in command
        assert command[command.index("--target") + 1] == "x86_64-unknown-linux-gnu"
        assert command[command.index("--bin") + 1] == inputs[role][-1][1]
        private_target = Path(command[command.index("--target-dir") + 1])
        assert private_target.parent == target
        rows = []
        for package, name, kind, filename in inputs[role]:
            package_path = root / "crates" / package
            artifact_dir = (private_target if fault != "foreign-artifact" else target) / "x86_64-unknown-linux-gnu" / "release"
            artifact_dir.mkdir(parents=True, exist_ok=True)
            artifact = artifact_dir / filename
            image = release_elf(package.encode(), machine=183 if fault == "wrong-target" else 62)
            artifact.write_bytes(image if kind == "bin" else package.encode())
            if kind == "bin":
                artifact.chmod(0o755)
            rows.append({
                "reason": "compiler-artifact", "package_id": f"path+{package_path.as_uri()}#0.1.0",
                "manifest_path": str(package_path / "Cargo.toml"),
                "target": {"name": name, "kind": [kind]}, "profile": {"test": False, "opt_level": "3"},
                "features": [], "executable": str(artifact) if kind == "bin" else None,
                "filenames": [str(artifact)],
            })
        rows.append({"reason": "build-finished", "success": True})
        kwargs["stdout"].write("".join(json.dumps(row) + "\n" for row in rows).encode())
        if fault == "source-drift":
            source.write_text("[workspace]\n# changed during build\n")
        if fault == "source-restored":
            original = source.read_bytes()
            timestamps = source.stat()
            source.write_bytes(b"[workspace]\n# changed during build\n")
            source.write_bytes(original)
            os.utime(source, ns=(timestamps.st_atime_ns, timestamps.st_mtime_ns))
        return subprocess.CompletedProcess(command, 101 if fault == "compiler-exit" else 0)

    monkeypatch.setattr(checker, "cargo_output", metadata)
    monkeypatch.setattr(checker.subprocess, "run", run)
    failures = {
        "source-drift": (ClosureError, "refusing dirty relevant source"),
        "source-restored": (ValueError, "release build source changed during compilation"),
        "compiler-exit": (subprocess.CalledProcessError, "exit status 101"),
        "foreign-artifact": (ValueError, "must come from this invocation's target directory"),
        "wrong-target": (ValueError, "Linux x86-64 executable"),
    }
    if fault is not None:
        error, message = failures[fault]
        with pytest.raises(error, match=message):
            checker.capture_release_build(root, role, evidence)
        assert not (evidence / "build-result.json").exists()
        return
    assert checker.capture_release_build(root, role, evidence) == []
    digest = checker.file_sha256(evidence / "build-result.json")
    evidence = evidence.rename(tmp_path / "relocated release evidence")
    if role == "daemon":
        findings, binary_digest, captured = checker.release_build_findings(
            evidence, digest, installed_linux=True,
        )
        assert binary_digest == hashlib.sha256(release_elf(inputs[role][0][0].encode())).hexdigest()
    else:
        findings, captured = checker.release_cli_findings(evidence, digest)
    assert findings == []
    assert json.loads(captured) == checker.capture_release_source(root)
    assert not list(target.glob(".tantivy-release-*"))
    if role == "daemon":
        assert checker.select_captured_daemon(root, evidence, digest) == (
            binary_digest, evidence / "quanta-index-searchd",
        )
        source.write_text("[workspace]\n# next release\n")
        real_run(
            ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "-qam", "next"],
            cwd=root, check=True,
        )
        with pytest.raises(ValueError, match="source files differ"):
            checker.select_captured_daemon(root, evidence, digest)


def test_retirement_recipes_install_locked_dependencies_without_building_the_checkout():
    checker = load_checker()
    for recipe, required_flags in (
        ("rust-check-tantivy-retirement", ("--check-migrations",)),
        ("rust-check-tantivy-shipment", ("--proof-checkout", "--installed-readback", "--installed-q-approval-request")),
    ):
        command = subprocess.run(
            ["just", "--dry-run", recipe], cwd=checker.ROOT, check=True,
            capture_output=True, text=True,
        ).stderr
        sync, invoke = command.split("uv run ", 1)
        assert "uv sync " in sync
        assert {"--frozen", "--offline", "--no-install-project"} <= set(sync.split())
        assert "--no-sync" in invoke.split()
        assert "tools/ci/check-tantivy-retirement.py" in invoke
        assert all(flag in invoke for flag in required_flags)


@pytest.mark.parametrize("revision", [None, "a" * 40])
@pytest.mark.parametrize(("statuses", "expected"), [((1, 0), 1), ((0, 1), 1), ((1, 2), 2), ((0, 0), 0)])
def test_retirement_checks_both_dispositions(checkout, monkeypatch, revision, statuses, expected):
    checker, _ = checkout
    root = checker.ROOT
    calls = []

    def run(argv, *, cwd, check):
        assert cwd == root and check is False
        calls.append((Path(argv[1]).name, argv[2:]))
        return subprocess.CompletedProcess(argv, statuses[len(calls) - 1])

    monkeypatch.setattr(checker.subprocess, "run", run)
    result = checker.check_migration_dispositions(root, revision)
    committed_input = ["--revision", revision] if revision is not None else []
    assert calls == [
        ("check-lexical-test-disposition.py", committed_input),
        ("check-runtime-legacy-migration.py", committed_input or ["--require-execution"]),
    ]
    assert result == expected


@pytest.mark.parametrize(
    ("recipe", "expected_proof_steps"),
    (
        ("proof-authority-code-gate", ["check-proof-authority.py"]),
        ("proof-authority-final-qualification", ["retirement --publish-aggregate", "check-proof-authority.py"]),
        ("proof-authority-release-gate", ["retirement --require-aggregate"]),
    ),
)
@pytest.mark.parametrize("retirement_exit", (0, 1, 2))
def test_release_gates_require_retirement_before_proof_steps(
    tmp_path, recipe, expected_proof_steps, retirement_exit,
):
    justfile = (load_checker().ROOT / "Justfile").read_text()
    # Keep recipe commands intact while preventing a login shell from changing PATH.
    (tmp_path / "Justfile").write_text(justfile.replace(
        'set shell := ["/bin/zsh", "-lc"]', 'set shell := ["/bin/sh", "-c"]', 1,
    ))
    (tmp_path / "uv").write_text(
        '#!/bin/sh\nif [ "$1" = sync ]; then exit 0; fi\n'
        'case " $* " in *" --proof-checkout "*) ;; *) exit 91 ;; esac\n'
        'case " $* " in *" --installed-readback signed-readback "*) ;; *) exit 92 ;; esac\n'
        'case " $* " in *" --installed-q-approval-request q-approval-request "*) ;; *) exit 93 ;; esac\n'
        'for arg; do mode="$arg"; done\n'
        'printf "retirement %s\\n" "$mode"\nexit "$RETIREMENT_EXIT"\n'
    )
    (tmp_path / "python3").write_text('#!/bin/sh\nprintf "%s\\n" "${1##*/}"\n')
    for name in ("uv", "python3"):
        (tmp_path / name).chmod(0o755)
    result = subprocess.run(
        ["just", recipe],
        cwd=tmp_path,
        env={
            **os.environ,
            "PATH": f"{tmp_path}{os.pathsep}{os.environ['PATH']}",
            "SEMANTICA_CHECKOUT": str(tmp_path / "semantica"),
            "RETIREMENT_EXIT": str(retirement_exit),
            "QUANTA_RETIREMENT_DAEMON_BUILD": "daemon-evidence",
            "QUANTA_RETIREMENT_DAEMON_RECEIPT_SHA256": "daemon-digest",
            "QUANTA_RETIREMENT_CLI_BUILD": "cli-evidence",
            "QUANTA_RETIREMENT_CLI_RECEIPT_SHA256": "cli-digest",
            "QUANTA_RETIREMENT_INSTALLED_BUNDLE": "signed-bundle",
            "QUANTA_RETIREMENT_INSTALLED_READBACK": "signed-readback",
            "QUANTA_RETIREMENT_INSTALLED_Q_APPROVAL_REQUEST": "q-approval-request",
        },
        check=False,
        capture_output=True,
        text=True,
    )
    expected_exit = 0 if recipe == "proof-authority-code-gate" else retirement_exit
    assert result.returncode == expected_exit
    expected = expected_proof_steps if expected_exit == 0 else expected_proof_steps[:1]
    assert result.stdout.splitlines() == expected


@pytest.mark.parametrize("outcome", [
    "valid", "failed-proof", "changed-proof", "different-daemon", "valid-aggregate", "changed-action",
])
def test_shipment_derives_authority_from_unchanged_current_proofs(tmp_path, monkeypatch, outcome):
    checker = load_checker()
    registry = tmp_path / "tools/ci/proof-authority.toml"
    registry.parent.mkdir(parents=True)
    registry.write_text(
        '[[proofs]]\nid = "p10-state-migration"\nartifact = "p10.json"\n'
        '[[proofs]]\nid = "p11-cross-repo-cutover"\nartifact = "p11.json"\n'
        'paired_repository = "github:josongmin/semantica-codegraph-v2"\n'
    )
    daemon = "a" * 64
    semantica_head = "b" * 40
    quanta_head = "d" * 40
    (tmp_path / "p10.json").write_text(json.dumps({
        "daemon_binary": {"sha256": daemon}, "source": {"head": quanta_head},
    }))
    p11 = tmp_path / "p11.json"
    p11.write_text(json.dumps({
        "daemon_binary": {"sha256": "c" * 64 if outcome == "different-daemon" else daemon},
        "source_pair": {"source": {"head": semantica_head}},
    }))
    require_aggregate = outcome in {"valid-aggregate", "changed-action"}
    target = {
        "state_root": "/var/lib/quanta/native",
        "config_sha256": "2" * 64,
        "release_manifest_sha256": "3" * 64,
    }
    if require_aggregate:
        with registry.open("a") as stream:
            for action in ("deployment", "activation", "rollback"):
                stream.write(f'[[proofs]]\nid = "p11-{action}"\nartifact = "{action}.json"\n')
                (tmp_path / f"{action}.json").write_text(json.dumps({
                    "execution_result": {"kind": "operational", "target": target},
                }))
    paired_checkout = tmp_path / "Semantica checkout"

    def check_proofs(argv, *, cwd, check):
        assert cwd == tmp_path and check
        assert ("--require-all" if require_aggregate else "--require-code-qualified") in argv
        assert "--bind-source" in argv
        assert f"github:josongmin/semantica-codegraph-v2={paired_checkout}" in argv
        if outcome == "failed-proof":
            raise subprocess.CalledProcessError(1, argv)
        if outcome == "changed-proof":
            p11.write_text("{}")
        if outcome == "changed-action":
            (tmp_path / "rollback.json").write_text("{}")

    monkeypatch.setattr(checker.subprocess, "run", check_proofs)
    if outcome == "failed-proof":
        with pytest.raises(subprocess.CalledProcessError):
            checker.proof_bound_candidate(tmp_path, paired_checkout)
    elif outcome in {"changed-proof", "changed-action"}:
        with pytest.raises(ValueError, match="changed"):
            checker.proof_bound_candidate(tmp_path, paired_checkout, require_aggregate=require_aggregate)
    elif outcome == "different-daemon":
        with pytest.raises(ValueError, match="different daemon"):
            checker.proof_bound_candidate(tmp_path, paired_checkout)
    else:
        installation = (
            target["state_root"], target["config_sha256"], target["release_manifest_sha256"],
        ) if require_aggregate else None
        assert checker.proof_bound_candidate(tmp_path, paired_checkout, require_aggregate=require_aggregate) == (
            (daemon, semantica_head, quanta_head), installation,
        )


def test_final_release_source_requires_complete_current_tree(tmp_path):
    checker = load_checker()
    for name in ("Cargo.lock", "src.rs"):
        (tmp_path / name).write_bytes(name.encode())
    for argv in (
        ("init", "-q"),
        ("add", "."),
        ("-c", "user.name=Gate Test", "-c", "user.email=gate@example.test", "commit", "-qm", "fixture"),
    ):
        subprocess.run(["git", *argv], cwd=tmp_path, check=True)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=tmp_path, text=True).strip()
    expected = {name: hashlib.sha256(name.encode()).hexdigest() for name in ("Cargo.lock", "src.rs")}
    assert checker.capture_release_source(tmp_path) == {"files": expected}
    checker.require_release_source(tmp_path, json.dumps({"files": expected}).encode(), head)
    for observed in ({"Cargo.lock": expected["Cargo.lock"]}, {**expected, "src.rs": "0" * 64}):
        with pytest.raises(ValueError, match="complete P10 checkout"):
            checker.require_release_source(tmp_path, json.dumps({"files": observed}).encode(), head)
    with pytest.raises(ValueError, match="HEAD differs"):
        checker.capture_release_source(tmp_path, "0" * 40)
    (tmp_path / "src.rs").write_text("dirty")
    with pytest.raises(RuntimeError, match="dirty relevant source"):
        checker.capture_release_source(tmp_path, head)


@pytest.mark.parametrize("visibility", ["--assume-unchanged", "--skip-worktree", "replace-ref"])
def test_final_release_source_rejects_hidden_cargo_lock_change(tmp_path, visibility):
    checker = load_checker()
    lock = tmp_path / "Cargo.lock"
    lock.write_text("version = 4\n")

    def git(*args):
        return subprocess.check_output(
            ["git", "-c", "user.name=Gate Test", "-c", "user.email=gate@example.test", *args],
            cwd=tmp_path, text=True,
        ).strip()

    git("init", "-q")
    git("add", "Cargo.lock")
    git("commit", "-qm", "candidate")
    head = git("rev-parse", "HEAD")
    if visibility != "replace-ref":
        git("update-index", visibility, "Cargo.lock")
    lock.write_text("version = 5\n")
    if visibility == "replace-ref":
        git("commit", "-qam", "replacement without candidate authority")
        git("replace", head, "HEAD")
        git("update-ref", "HEAD", head)
    assert git("status", "--porcelain") == ""
    with pytest.raises(RuntimeError, match="differs from committed HEAD: Cargo.lock"):
        checker.capture_release_source(tmp_path, head)


def test_final_shipment_reuses_canonical_s_product_validation(tmp_path):
    checker = load_checker()
    ci = tmp_path / "tools/ci"
    product = tmp_path / "tools/daemon_release_manifest"
    ci.mkdir(parents=True)
    product.mkdir()
    (ci / "source_snapshot_identity_v1.py").write_text(
        "def source_snapshot_digest_v1(*, cwd):\n    return 'a' * 64\n"
    )
    (product / "product_bundle.py").write_text(
        "LINUX_INSTALLED_PRODUCT_BUILD_PROFILE_V1 = 'installed'\n"
        "def verify_product_bundle_v1(root, *, expected_profile_id_v1):\n"
        "    assert expected_profile_id_v1 == 'installed'\n"
        "    assert (root / 'surface-evidence.json').is_file()\n"
    )
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    surface = bundle / "surface-evidence.json"
    surface.touch()
    checker.verify_paired_product_bundle(tmp_path, bundle, "a" * 64)
    with pytest.raises(subprocess.CalledProcessError):
        checker.verify_paired_product_bundle(tmp_path, bundle, "b" * 64)
    surface.unlink()
    with pytest.raises(subprocess.CalledProcessError):
        checker.verify_paired_product_bundle(tmp_path, bundle, "a" * 64)


def test_independent_dependency_and_vendor_residue(checkout, capsys):
    checker, cargo = checkout
    cargo["tree"] += "tantivy-fst v0.5.0\n"
    metadata = json.loads(cargo["metadata"])
    metadata["packages"].append({"id": "dependency", "name": "tantivy-columnar", "features": {}})
    cargo["metadata"] = json.dumps(metadata)
    lock = checker.ROOT / "Cargo.lock"
    lock.write_text(lock.read_text() + '[[package]]\nname = "tantivy_common"\n')
    (checker.ROOT / "Cargo.toml").write_text(
        '[workspace]\nexclude = ["vendor/tantivy-old"]\n'
        '[workspace.dependencies]\nsearch = { package = "tantivy", version = "0.22" }\n'
        '[patch.crates-io]\ntantivy = { path = "vendor/renamed" }\n'
    )
    renamed = checker.ROOT / "vendor/renamed"
    renamed.mkdir(parents=True)
    (renamed / "Cargo.toml").write_text('[package]\nname = "tantivy"\n')
    assert checker.main() == 1
    assert capsys.readouterr().err.splitlines() == [
        "Cargo tree: tantivy-fst",
        "Cargo metadata: tantivy-columnar",
        "Cargo.lock: tantivy_common",
        "Cargo.toml workspace.dependencies: search",
        "Cargo.toml patch.crates-io: tantivy",
        "Cargo.toml workspace.exclude: vendor/tantivy-old",
        "vendor: vendor/renamed",
    ]


def test_disconnected_rust_sources_block_retirement(checkout, capsys):
    checker, _ = checkout
    sources = checker.ROOT / "crates" / "unregistered" / "src"
    sources.mkdir(parents=True)
    (sources / "alternate.rs").write_text("extern crate tantivy_fst as old_fst;\n")
    (sources / "legacy.rs").write_text("use tantivy /* old engine */ ::schema::Field;\n")
    (sources / "format.rs").write_text(
        '/* Preserve the old envelope. /* use tantivy::Index; */ tantivy::schema */\n'
        'const FORMAT: &str = r#"tantivy::DocAddress"#;\n'
        '// use tantivy_fst::Automaton;\n'
        'use internal::{tantivy as local_type};\n'
    )
    assert checker.main() == 1
    assert capsys.readouterr().err.splitlines() == [
        "Rust source: crates/unregistered/src/alternate.rs",
        "Rust source: crates/unregistered/src/legacy.rs",
    ]


def test_nested_vendor_package_blocks_retirement(checkout, capsys):
    checker, _ = checkout
    bundle = checker.ROOT / "vendor" / "bundle"
    nested = bundle / "renamed"
    nested.mkdir(parents=True)
    (bundle / "Cargo.toml").write_text('[workspace]\nmembers = ["renamed"]\n')
    (nested / "Cargo.toml").write_text('[package]\nname = "tantivy-fst"\n')
    assert checker.main() == 1
    assert capsys.readouterr().err.splitlines() == ["vendor: vendor/bundle/renamed"]


@pytest.mark.parametrize(
    "statement",
    (
        "use {std::fmt, tantivy as old};",
        "use ::{tantivy_fst as old};",
        "use {tantivy::*, std::fmt};",
        "extern crate r#tantivy as old;",
        "use r#tantivy_fst as old;",
    ),
)
def test_retirement_detects_grouped_and_raw_crate_imports(checkout, capsys, statement):
    checker, _ = checkout
    (checker.ROOT / "unmounted.rs").write_text(statement)
    assert checker.main() == 1
    assert capsys.readouterr().err == "Rust source: unmounted.rs\n"


def test_nested_vendor_link_cannot_hide_a_package(checkout, capsys):
    checker, _ = checkout
    package = checker.ROOT / "retained" / "renamed"
    package.mkdir(parents=True)
    (package / "Cargo.toml").write_text('[package]\nname = "tantivy-fst"\n')
    bundle = checker.ROOT / "vendor" / "bundle"
    bundle.mkdir(parents=True)
    (bundle / "renamed").symlink_to(package, target_is_directory=True)
    assert checker.main() == 1
    assert capsys.readouterr().err == "vendor: vendor/bundle/renamed\n"


def test_retirement_detects_crate_paths_in_macro_tokens(checkout, capsys):
    checker, _ = checkout
    (checker.ROOT / "unmounted.rs").write_text("quote! { use tantivy /* old */ ::Index; }")
    assert checker.main() == 1
    assert capsys.readouterr().err == "Rust source: unmounted.rs\n"


def test_workspace_legacy_tantivy_feature_blocks_retirement_without_a_dependency(checkout, capsys):
    checker, cargo = checkout
    metadata = json.loads(cargo["metadata"])
    metadata["packages"][0]["features"]["legacy-tantivy"] = []
    cargo["metadata"] = json.dumps(metadata)
    assert checker.main() == 1
    assert capsys.readouterr().err.splitlines() == [
        "Cargo workspace feature: quanta-index-searchd-runtime/legacy-tantivy"
    ]


def test_legacy_bench_feature_still_blocks_retirement_after_dependency_removal(checkout, capsys):
    checker, cargo = checkout
    metadata = json.loads(cargo["metadata"])
    metadata["workspace_members"].extend(["harness-package", "unrelated-package"])
    metadata["packages"].extend([
        {
            "id": "harness-package",
            "name": "quanta-index-searchd-harness",
            "features": {"legacy-bench": [], "semantic-bench": []},
        },
        {"id": "unrelated-package", "name": "unrelated", "features": {"legacy-bench": []}},
    ])
    cargo["metadata"] = json.dumps(metadata)
    assert checker.main() == 1
    assert capsys.readouterr().err.splitlines() == [
        "Cargo workspace feature: quanta-index-searchd-harness/legacy-bench"
    ]


def test_retirement_feature_check_is_scoped_to_workspace_members(checkout):
    checker, cargo = checkout
    metadata = json.loads(cargo["metadata"])
    metadata["packages"].append({
        "id": "unrelated",
        "name": "unrelated-package",
        "features": {"legacy-tantivy": []},
    })
    cargo["metadata"] = json.dumps(metadata)
    assert checker.main() == 0


@pytest.mark.parametrize(
    ("command", "response"),
    [("tree", ""), ("metadata", '{"packages": []}'), ("metadata", "{broken")],
)
def test_empty_or_malformed_resolution_is_unavailable(checkout, command, response):
    checker, cargo = checkout
    cargo[command] = response
    assert checker.main() == 2


def record_release_stream(checker, evidence, events):
    artifacts = evidence / "build-artifacts.jsonl"
    artifacts.write_text("".join(json.dumps(row) + "\n" for row in events))
    receipt_path = evidence / "build-result.json"
    receipt = json.loads(receipt_path.read_bytes())
    receipt["compiler_stream_sha256"] = checker.file_sha256(artifacts)
    receipt_path.write_text(json.dumps(receipt))
    return checker.file_sha256(receipt_path)


@pytest.fixture
def release_build(checkout):
    checker, cargo = checkout
    evidence = checker.ROOT / "captured-release"
    evidence.mkdir()
    compiler_path = checker.ROOT / "target/release" / checker.DAEMON_BINARY
    compiler_path.parent.mkdir(parents=True)
    compiler_path.write_bytes(release_elf(b"compiler output bytes"))
    binary = evidence / checker.DAEMON_BINARY
    binary.write_bytes(compiler_path.read_bytes())
    package_directory = checker.ROOT / "crates" / checker.WORKSPACE_PACKAGE
    daemon_id = f"path+{package_directory.as_uri()}#0.1.0"
    cargo["metadata"] = json.dumps({
        "target_directory": str(checker.ROOT / "target"),
        "build_directory": str(checker.ROOT / "build-cache"),
    })
    receipt = {
        "status": "VERIFIED",
        "exit_code": 0,
        "source_before_after_equal": True,
        "command": [
            "./scripts/cargow", "build", "--release", "--locked", "--offline",
            "--message-format=json-render-diagnostics",
        ],
        "binaries": {checker.DAEMON_BINARY: {
            "path": str(binary),
            "compiler_artifact": str(compiler_path),
            "compiler_artifact_sha256": checker.file_sha256(compiler_path),
            "sha256": checker.file_sha256(binary),
        }},
    }
    for name in ("source-before.json", "source-after-build.json"):
        (evidence / name).write_text(json.dumps({"files": {"Cargo.toml": "a" * 64}}))
    receipt["source_capture_sha256"] = checker.file_sha256(evidence / "source-before.json")
    (evidence / "build-result.json").write_text(json.dumps(receipt))
    events = [
        {
            "reason": "compiler-artifact",
            "package_id": daemon_id,
            "manifest_path": str(package_directory / "Cargo.toml"),
            "target": {"name": checker.DAEMON_BINARY, "kind": ["bin"]},
            "profile": {"test": False, "opt_level": "3"},
            "features": [],
            "executable": str(compiler_path),
        },
        {"reason": "build-finished", "success": True},
    ]
    return checker, evidence, binary, events


def test_native_release_rejects_existing_shared_daemon(release_build, monkeypatch, capsys):
    checker, _, _, events = release_build
    shared_daemon = Path(events[0]["executable"])
    shared_daemon.write_bytes(b"previous build")
    shared_daemon.chmod(0o700)

    def run(command, *, cwd, stdout, check, **kwargs):
        stdout.write("".join(json.dumps(row) + "\n" for row in events).encode())
        return checker.subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(checker.subprocess, "run", run)
    assert checker.main(build_native_daemon=True) == 2
    assert shared_daemon.read_bytes() == b"previous build"
    assert not capsys.readouterr().out


@pytest.mark.parametrize("relocated", [False, True])
def test_observed_release_is_independent_of_live_checkout(
    release_build, monkeypatch, capsys, relocated
):
    checker, evidence, binary, events = release_build
    historical_package = (
        evidence.parent / "build source #1" / "crates" / checker.WORKSPACE_PACKAGE
    )
    events[0]["package_id"] = f"path+{historical_package.as_uri()}#0.1.0"
    events[0]["manifest_path"] = str(historical_package / "Cargo.toml")
    receipt_sha256 = record_release_stream(checker, evidence, events)
    installed_sha256 = checker.file_sha256(binary)
    Path(events[0]["executable"]).unlink()
    if relocated:
        evidence = evidence.rename(evidence.parent / "relocated release")

    live_checkout = evidence.parent / "different-checkout"
    live_checkout.mkdir()
    monkeypatch.setattr(checker, "ROOT", live_checkout)
    monkeypatch.setattr(
        checker, "cargo_output", lambda *_: pytest.fail("archive queried live Cargo metadata")
    )
    assert checker.main(
        evidence, installed_sha256, expected_build_receipt_sha256=receipt_sha256
    ) == 0
    assert installed_sha256 in capsys.readouterr().out


@pytest.mark.parametrize(("package_id", "name"), [
    ("path+file:///old/vendor/other-folder#tantivy@0.22.1", "tantivy"),
    ("path+file:///old/vendor/tantivy-fst#0.5.0", "tantivy-fst"),
    ("git+ssh://git@example.org/repo?branch=old#tantivy-common@0.7.0", "tantivy-common"),
    ("sparse+https://index.crates.io/#tantivy-stacker@0.3.0", "tantivy-stacker"),
])
def test_archive_checks_canonical_package_name(release_build, capsys, package_id, name):
    checker, evidence, binary, events = release_build
    events.insert(0, {"reason": "compiler-artifact", "package_id": package_id})
    receipt_sha256 = record_release_stream(checker, evidence, events)
    assert checker.main(
        evidence, checker.file_sha256(binary), expected_build_receipt_sha256=receipt_sha256
    ) == 1
    assert capsys.readouterr().err.splitlines() == [f"Release compiler artifact: {name}"]


@pytest.mark.parametrize(("receipt_update", "files"), [
    ({"exit_code": False}, None),
    ({"exit_code": 0.0}, None),
    ({"command": ["build", "--release", "--locked", "--offline",
                  "--message-format=json-render-diagnostics", None]}, None),
    ({}, ["Cargo.toml"]),
    ({}, {"Cargo.toml": "not-a-sha"}),
    ({}, {"../Cargo.toml": "a" * 64}),
])
def test_archive_rejects_pinned_malformed_receipt_and_capture(
    release_build, capsys, receipt_update, files
):
    checker, evidence, binary, events = release_build
    receipt_path = evidence / "build-result.json"
    receipt = json.loads(receipt_path.read_bytes())
    receipt.update(receipt_update)
    if files is not None:
        before = evidence / "source-before.json"
        capture = json.dumps({"files": files})
        before.write_text(capture)
        (evidence / "source-after-build.json").write_text(capture)
        receipt["source_capture_sha256"] = checker.file_sha256(before)
    receipt_path.write_text(json.dumps(receipt))
    receipt_sha256 = record_release_stream(checker, evidence, events)
    assert checker.main(
        evidence, checker.file_sha256(binary), expected_build_receipt_sha256=receipt_sha256
    ) == 2
    assert not capsys.readouterr().out


@pytest.mark.parametrize("compiler_digest", [None, "a" * 64, "malformed"])
def test_archive_requires_original_compiler_output_digest(release_build, capsys, compiler_digest):
    checker, evidence, binary, events = release_build
    receipt_path = evidence / "build-result.json"
    receipt = json.loads(receipt_path.read_bytes())
    daemon = receipt["binaries"][checker.DAEMON_BINARY]
    if compiler_digest is None:
        del daemon["compiler_artifact_sha256"]
    else:
        daemon["compiler_artifact_sha256"] = compiler_digest
    receipt_path.write_text(json.dumps(receipt))
    receipt_sha256 = record_release_stream(checker, evidence, events)
    assert checker.main(
        evidence, checker.file_sha256(binary), expected_build_receipt_sha256=receipt_sha256
    ) == 2
    output = capsys.readouterr()
    assert not output.out
    assert "compiler artifact SHA-256" in output.err


@pytest.mark.parametrize("fault, expected", [
    (None, 0),
    ("tantivy", 1),
    ("missing_terminal", 2),
    ("wrong_executable", 2),
    ("wrong_manifest", 2),
    ("malformed_package_id", 2),
    ("opaque_package_id", 2),
    ("legacy_feature", 2),
    ("missing_features", 2),
    ("binary_changed", 2),
    ("binary_symlink", 2),
    ("source_changed", 2),
    ("both_sources_changed", 2),
])
def test_observed_release_compiler_packages(release_build, fault, expected, capsys):
    checker, evidence, binary, events = release_build
    installed_sha256 = checker.file_sha256(binary)
    if fault == "tantivy":
        events.insert(0, {"reason": "compiler-artifact", "package_id": TANTIVY_PACKAGE_ID})
    elif fault == "missing_terminal":
        events.pop()
    elif fault == "wrong_executable":
        events[0]["executable"] = str(evidence / "other-daemon")
    elif fault == "wrong_manifest":
        events[0]["manifest_path"] = str(
            evidence / "crates" / checker.WORKSPACE_PACKAGE / "Cargo.toml"
        )
    elif fault == "malformed_package_id":
        events.insert(0, {"reason": "compiler-artifact", "package_id": "path+file:///old#lib@"})
    elif fault == "opaque_package_id":
        events.insert(0, {
            "reason": "compiler-artifact", "package_id": "lib 1.0.0 (registry+https://example.org)"
        })
    elif fault == "legacy_feature":
        events[0]["features"] = ["legacy-tantivy"]
    elif fault == "missing_features":
        del events[0]["features"]
    elif fault == "binary_changed":
        binary.write_bytes(b"different daemon bytes")
    elif fault == "binary_symlink":
        retained = binary.rename(evidence.parent / "retained-daemon")
        binary.symlink_to(retained)
    elif fault == "source_changed":
        (evidence / "source-after-build.json").write_text('{"files":{"Cargo.toml":"different"}}')
    elif fault == "both_sources_changed":
        for name in ("source-before.json", "source-after-build.json"):
            (evidence / name).write_text('{"files":{"Cargo.toml":"different"}}')
    receipt_sha256 = record_release_stream(checker, evidence, events)
    assert checker.main(
        evidence, installed_sha256, expected_build_receipt_sha256=receipt_sha256
    ) == expected
    if fault == "tantivy":
        assert "Release compiler artifact: tantivy-fst" in capsys.readouterr().err


@pytest.mark.parametrize("expected", ["same", "different", "invalid"])
def test_release_matches_installed_daemon_digest(release_build, expected, capsys):
    checker, evidence, binary, events = release_build
    receipt_sha256 = record_release_stream(checker, evidence, events)
    actual = checker.file_sha256(binary)
    anchor = {"same": actual, "different": "0" * 64, "invalid": "missing"}[expected]
    status = checker.main(
        evidence, anchor, expected_build_receipt_sha256=receipt_sha256
    )
    observed = capsys.readouterr()
    if expected == "same":
        assert status == 0
        assert actual in observed.out
    elif expected == "different":
        assert status == 2
        assert "differs from expected" in observed.err
    else:
        assert status == 2
        assert "64 lowercase hex characters" in observed.err


def test_installed_digest_requires_release_evidence(checkout, capsys):
    checker, _ = checkout
    assert checker.main(expected_daemon_sha256="a" * 64) == 2
    assert "requires --release-build" in capsys.readouterr().err


def test_release_refuses_another_compiler_stream_for_the_same_artifact_path(release_build, capsys):
    checker, evidence, binary, events = release_build
    artifacts = evidence / "build-artifacts.jsonl"
    forbidden = {"reason": "compiler-artifact", "package_id": TANTIVY_PACKAGE_ID}
    original_receipt_sha256 = record_release_stream(checker, evidence, [forbidden, *events])
    installed_sha256 = checker.file_sha256(binary)
    assert checker.main(
        evidence, installed_sha256, expected_build_receipt_sha256=original_receipt_sha256
    ) == 1
    assert "Release compiler artifact: tantivy-fst" in capsys.readouterr().err

    # The replacement is a complete successful stream for the same executable path.
    artifacts.write_text("".join(json.dumps(row) + "\n" for row in events))
    assert checker.main(
        evidence, installed_sha256, expected_build_receipt_sha256=original_receipt_sha256
    ) == 2
    rejected = capsys.readouterr()
    assert not rejected.out
    assert "compiler stream SHA-256 differs" in rejected.err

    replacement_receipt_sha256 = record_release_stream(checker, evidence, events)
    assert replacement_receipt_sha256 != original_receipt_sha256
    assert checker.main(
        evidence, installed_sha256, expected_build_receipt_sha256=original_receipt_sha256
    ) == 2
    rejected = capsys.readouterr()
    assert not rejected.out
    assert "build receipt SHA-256 differs" in rejected.err


def test_archive_without_a_recorded_stream_digest_is_unavailable(release_build, capsys):
    checker, evidence, binary, events = release_build
    (evidence / "build-artifacts.jsonl").write_text(
        "".join(json.dumps(row) + "\n" for row in events)
    )
    assert checker.main(
        evidence,
        checker.file_sha256(binary),
        expected_build_receipt_sha256=checker.file_sha256(evidence / "build-result.json"),
    ) == 2
    assert "compiler_stream_sha256" in capsys.readouterr().err


@pytest.fixture
def shipped_release(release_build, tmp_path):
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    from tools.ci.tests.test_native_release_approval import selection_fixture
    from tools.deployment.native_release_approval import binding, signing_bytes

    checker, evidence, binary, events = release_build
    cli_evidence = checker.ROOT / "captured-cli-release"
    cli_evidence.mkdir()
    cli_events = []
    daemon_receipt_path = evidence / "build-result.json"
    daemon_receipt = json.loads(daemon_receipt_path.read_text())

    def command(lane, package, name, feature, target):
        return [
            str(checker.ROOT / "scripts/cargow"), "--lane", lane,
            "build", "-p", package, "--bin", name, feature, "--locked", "--offline", "--release",
            "--config", f'build.build-dir={json.dumps(str(checker.ROOT / "build-cache"))}',
            "--target-dir", str(target), "--message-format=json-render-diagnostics",
            "--target", "x86_64-unknown-linux-gnu",
        ]

    daemon_target = checker.ROOT / "target/.tantivy-release-daemon"
    compiler_daemon = daemon_target / "x86_64-unknown-linux-gnu" / "release" / checker.DAEMON_BINARY
    # The historical Cargo output directory no longer needs to exist.
    events[0]["executable"] = str(compiler_daemon)
    daemon_receipt["binaries"][checker.DAEMON_BINARY]["compiler_artifact"] = str(compiler_daemon)
    daemon_receipt["command"] = command(
        checker.NATIVE_RELEASE_LANE, checker.WORKSPACE_PACKAGE, checker.DAEMON_BINARY,
        "--no-default-features", daemon_target,
    )
    daemon_receipt_path.write_text(json.dumps(daemon_receipt))

    cli_target = checker.ROOT / "target/.tantivy-release-cli"
    cli_output = cli_target / "x86_64-unknown-linux-gnu" / "release"
    cli_output.mkdir(parents=True)
    cli_receipt = {**daemon_receipt, "command": command(
        checker.CLI_RELEASE_LANE, checker.CLI_BINARY, checker.CLI_BINARY, "--all-features", cli_target,
    )}
    cli_receipt["binaries"] = {}
    for name in ("source-before.json", "source-after-build.json"):
        (cli_evidence / name).write_bytes((evidence / name).read_bytes())
    for package, target, kind, filename in (
        ("quanta-index-sdk", "quanta_index_sdk", "lib", "libquanta_index_sdk-123.rlib"),
        ("quanta-index-searchctl", "quanta-index-searchctl", "bin", "quanta-index-searchctl"),
    ):
        package_dir = checker.ROOT / "crates" / package
        compiler = cli_output / filename
        compiler.write_bytes(release_elf(package.encode()) if kind == "bin" else package.encode())
        retained = cli_evidence / filename
        retained.write_bytes(compiler.read_bytes())
        cli_receipt["binaries"][package] = {
            "path": str(retained), "compiler_artifact": str(compiler),
            "compiler_artifact_sha256": checker.file_sha256(compiler),
            "sha256": checker.file_sha256(retained),
        }
        cli_events.append({
            "reason": "compiler-artifact",
            "package_id": f"path+{package_dir.as_uri()}#0.1.0",
            "manifest_path": str(package_dir / "Cargo.toml"),
            "target": {"name": target, "kind": [kind]},
            "profile": {"test": False, "opt_level": "3"},
            "features": [],
            "executable": str(compiler) if kind == "bin" else None,
            "filenames": [str(compiler)],
        })
    cli_events.append({"reason": "build-finished", "success": True})
    (cli_evidence / "build-result.json").write_text(json.dumps(cli_receipt))
    cli_receipt_sha = record_release_stream(checker, cli_evidence, cli_events)
    receipt_sha = record_release_stream(checker, evidence, events)
    install_root = tmp_path / "install"
    manifest = {
        "schema_version": "quanta-release-bundle-v4",
        "source_snapshot_digest": "a" * 64,
        "build_provenance": {"git_sha": "b" * 40, "source_snapshot_digest": "a" * 64},
        "build_receipt": {"relative_path": "share/quanta/build-receipt.json"},
        "artifacts": [],
    }
    members = {
        "public-cli": "bin/quanta",
        "server": "libexec/quanta/quanta-cli-server",
        "sql-rulepack-worker": "libexec/quanta/quanta-sql-rulepack-worker",
        "parser-native-worker": "libexec/quanta/quanta_native_worker_v3",
    }
    # The installer pins this manifest independently of Q's Cargo receipt.
    provisional = install_root / "releases" / "pending"
    provisional.mkdir(parents=True)
    for role, relative in members.items():
        member = provisional / relative
        member.parent.mkdir(parents=True, exist_ok=True)
        member.write_bytes(role.encode())
        member.chmod(0o755)
        manifest["artifacts"].append({
            "role": role, "relative_path": relative,
            "sha256": checker.file_sha256(member), "size_bytes": member.stat().st_size,
        })
    copied_receipt = provisional / "share/quanta/build-receipt.json"
    copied_receipt.parent.mkdir(parents=True, exist_ok=True)
    copied_receipt.write_text(json.dumps({"source_snapshot_digest": "a" * 64}))
    manifest["build_receipt"].update({
        "sha256": checker.file_sha256(copied_receipt), "size_bytes": copied_receipt.stat().st_size,
    })
    manifest_bytes = json.dumps(manifest, sort_keys=True).encode()
    manifest_sha = checker.hashlib.sha256(manifest_bytes).hexdigest()
    bundle = provisional.rename(install_root / "releases" / manifest_sha)
    release_manifest = bundle / "share/quanta/release-manifest.json"
    release_manifest.write_bytes(manifest_bytes)
    trust = tmp_path / "trust"
    trust.mkdir()
    active = {"payload": {
        "schema_version": "semantica-installed-product-active/v1",
        "install_root": str(install_root), "release_manifest_sha256": manifest_sha,
    }}
    active_bytes = json.dumps(active).encode()
    (trust / "active-generation.json").write_bytes(active_bytes)
    key = Ed25519PrivateKey.generate()
    (trust / "deployment-public-key.bin").write_bytes(key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    ))
    deployment = tmp_path / "q-deployment"
    deployment.mkdir(mode=0o700)
    selected_q, approval_request = selection_fixture(deployment, binary_bytes=binary.read_bytes())
    request_path = deployment / "approval-request.json"
    request_path.write_text(json.dumps(approval_request))
    payload = binding(
        selected_q, approval_request["selection_sha256"], approval_request["decision_policy_sha256"],
        checker.hashlib.sha256(active_bytes).hexdigest(), manifest_sha,
    )
    readback = tmp_path / "LOCAL-INSTALLED-LINUX-PRODUCED.json"
    readback.write_text(json.dumps({
        "state": "LOCAL_SIGNED_LINUX_INSTALLED_PRODUCT_PRODUCED_NOT_C01_ACCEPTANCE",
        "installed_active_generation_sha256": payload["active_generation_sha256"],
        "selected": active["payload"],
        "native_q_approval": {"payload": payload, "signature_hex": key.sign(signing_bytes(payload)).hex()},
        "original_Linux_C01_bodies": 0,
        "C01_acceptance": False,
    }))
    return (
        checker, evidence, binary, events, receipt_sha, cli_evidence, cli_events,
        cli_receipt_sha, bundle, readback, trust, request_path, selected_q,
    )


@pytest.mark.parametrize("fault", [
    None, "missing_sdk", "missing_cli", "tantivy", "swapped_member", "missing_member",
    "wrong_approval", "wrong_manifest", "wrong_source", "swapped_cli", "swapped_sdk",
    "wrong_s_head", "wrong_provenance", "cli_mach_o", "cli_aarch64",
    "readback_state", "readback_generation", "readback_selection", "standalone_approval", "swapped_selected_q",
])
def test_shipped_release_requires_compiled_consumers_and_approved_bundle(
    shipped_release, fault, capsys, monkeypatch,
):
    checker, evidence, binary, events, _, cli_evidence, cli_events, _, bundle, readback, trust, request, selected_q = shipped_release
    if fault in {"missing_sdk", "missing_cli", "tantivy"}:
        if fault == "missing_sdk":
            cli_events[:] = [row for row in cli_events if row.get("target", {}).get("name") != "quanta_index_sdk"]
        elif fault == "missing_cli":
            cli_events[:] = [row for row in cli_events if row.get("target", {}).get("name") != "quanta-index-searchctl"]
        else:
            cli_events.insert(-1, {"reason": "compiler-artifact", "package_id": "registry+https://index.crates.io/#tantivy@0.22.1"})
    elif fault in {"swapped_member", "missing_member"}:
        member = bundle / "bin/quanta"
        if fault == "swapped_member":
            member.write_bytes(b"swapped")
        else:
            member.unlink()
    elif fault == "swapped_selected_q":
        selected_q.binary.write_bytes(release_elf(b"other Q ELF"))
    elif fault == "wrong_approval":
        issued = json.loads(readback.read_text())
        issued["native_q_approval"]["payload"]["q_elf_sha256"] = "0" * 64
        readback.write_text(json.dumps(issued))
    elif fault in {"readback_state", "readback_generation", "readback_selection", "standalone_approval"}:
        issued = json.loads(readback.read_text())
        if fault == "readback_state":
            issued["state"] = "SIGNING_OR_INSTALL_FAILED"
        elif fault == "readback_generation":
            issued["installed_active_generation_sha256"] = "0" * 64
        elif fault == "readback_selection":
            issued["selected"]["install_root"] = "/another-install"
        else:
            issued = issued["native_q_approval"]
        readback.write_text(json.dumps(issued))
    elif fault == "wrong_manifest":
        (bundle / "share/quanta/release-manifest.json").write_bytes(b"{}")
    elif fault == "wrong_provenance":
        (bundle / "share/quanta/build-receipt.json").write_text(json.dumps({"source_snapshot_digest": "0" * 64}))
    elif fault == "wrong_source":
        (cli_evidence / "source-before.json").write_text('{"files":{"Cargo.toml":"' + "b" * 64 + '"}}')
        (cli_evidence / "source-after-build.json").write_bytes((cli_evidence / "source-before.json").read_bytes())
        cli_receipt = json.loads((cli_evidence / "build-result.json").read_text())
        cli_receipt["source_capture_sha256"] = checker.file_sha256(cli_evidence / "source-before.json")
        (cli_evidence / "build-result.json").write_text(json.dumps(cli_receipt))
    elif fault in {"swapped_cli", "swapped_sdk"}:
        filename = "quanta-index-searchctl" if fault == "swapped_cli" else "libquanta_index_sdk-123.rlib"
        (cli_evidence / filename).write_bytes(b"swapped")
    elif fault in {"cli_mach_o", "cli_aarch64"}:
        archived = cli_evidence / "quanta-index-searchctl"
        archived.write_bytes(b"\xcf\xfa\xed\xfe" + bytes(116) if fault == "cli_mach_o" else release_elf(machine=183))
        receipt_path = cli_evidence / "build-result.json"
        receipt = json.loads(receipt_path.read_bytes())
        entry = receipt["binaries"]["quanta-index-searchctl"]
        entry["sha256"] = entry["compiler_artifact_sha256"] = checker.file_sha256(archived)
        receipt_path.write_text(json.dumps(receipt))
    receipt_sha = record_release_stream(checker, evidence, events)
    cli_receipt_sha = record_release_stream(checker, cli_evidence, cli_events)
    s_head = "0" * 40 if fault == "wrong_s_head" else "b" * 40
    identity = (checker.file_sha256(binary), s_head, "d" * 40)
    installation = (str(selected_q.state_root), selected_q.native_config_sha256, bundle.name)
    monkeypatch.setattr(checker, "proof_bound_candidate", lambda *args, **kw: (identity, installation))
    monkeypatch.setattr(checker, "require_release_source", lambda *_: None)
    monkeypatch.setattr(checker, "check_migration_dispositions", lambda *_: 0)
    monkeypatch.setattr(checker, "verify_paired_product_bundle", lambda *_: None)
    monkeypatch.setattr(checker, "retirement_findings", lambda *_: [])
    result = checker.main(
        evidence, expected_build_receipt_sha256=receipt_sha,
        release_cli_build=cli_evidence, expected_cli_build_receipt_sha256=cli_receipt_sha,
        shipped_bundle=bundle, installed_readback=readback, trust_root=trust,
        installed_q_approval_request=request,
        proof_checkout=checker.ROOT / "s",
    )
    assert result == (0 if fault is None else 1 if fault == "tantivy" else 2)
    if fault in {"cli_mach_o", "cli_aarch64"}:
        assert "Linux x86-64 executable" in capsys.readouterr().err
    if fault in {"readback_state", "readback_generation", "readback_selection", "standalone_approval"}:
        assert "signed install readback differs" in capsys.readouterr().err
    if fault == "swapped_selected_q":
        assert "selected binary SHA-256 changed" in capsys.readouterr().err


def test_signed_bundle_cannot_qualify_without_committed_candidate_authority(shipped_release, capsys):
    checker, evidence, binary, _, receipt_sha, cli, _, cli_sha, bundle, readback, trust, *_ = shipped_release
    assert checker.main(
        evidence, checker.file_sha256(binary), expected_build_receipt_sha256=receipt_sha,
        release_cli_build=cli, expected_cli_build_receipt_sha256=cli_sha,
        shipped_bundle=bundle, installed_readback=readback, trust_root=trust,
    ) == 2
    assert "requires --proof-checkout to select committed Q/S and test authority" in capsys.readouterr().err


@pytest.mark.parametrize("role", ["daemon", "cli"])
@pytest.mark.parametrize("field", ["features", "target", "output", "source"])
def test_installed_release_rejects_wrong_compiler_invocation(shipped_release, role, field):
    checker, daemon, _, _, _, cli, _, _, *_ = shipped_release
    evidence = daemon if role == "daemon" else cli
    path = evidence / "build-result.json"
    receipt = json.loads(path.read_text())
    command = receipt["command"]
    if field == "features":
        feature = "--no-default-features" if role == "daemon" else "--all-features"
        command.remove(feature)
    elif field == "target":
        command[command.index("--target") + 1] = "aarch64-unknown-linux-gnu"
    elif field == "output":
        command[command.index("--target-dir") + 1] = str(checker.ROOT / "target/.tantivy-release-other")
    else:
        command[0] = str(checker.ROOT / "another-source/scripts/cargow")
    path.write_text(json.dumps(receipt))
    digest = checker.file_sha256(path)
    with pytest.raises(ValueError):
        if role == "daemon":
            checker.release_build_findings(evidence, digest, installed_linux=True)
        else:
            checker.release_cli_findings(evidence, digest)


@pytest.mark.parametrize(
    ("residue", "installed_change", "disposition_exit"),
    [(False, None, 0), (True, None, 0), (False, "readback", 0), (False, "q-elf", 0), (False, None, 1)],
)
def test_final_qualification_only_publishes_the_checked_candidate(
    shipped_release, monkeypatch, capsys, residue, installed_change, disposition_exit,
):
    checker, evidence, binary, _, receipt_sha, cli, _, cli_sha, bundle, readback, trust, request, selected_q = shipped_release
    paired_checkout = checker.ROOT / "s"
    identity = (checker.file_sha256(binary), "b" * 40, "d" * 40)
    events = []
    monkeypatch.setattr(
        checker, "proof_bound_candidate",
        lambda root, checkout, *, require_aggregate: (identity, None),
    )
    monkeypatch.setattr(checker, "require_release_source", lambda *args: events.append("source"))

    def verify_bundle(*_args):
        events.append("bundle")
        if installed_change == "readback":
            readback.write_bytes(readback.read_bytes() + b"\n")
        elif installed_change == "q-elf":
            selected_q.binary.write_bytes(release_elf(b"other Q ELF"))

    monkeypatch.setattr(checker, "verify_paired_product_bundle", verify_bundle)

    def check_workspace(root):
        events.append("workspace")
        return ["vendor: vendor/tantivy"] if residue else []

    def publish(checkout, observed_identity, installation):
        assert checkout == paired_checkout and observed_identity == identity
        assert installation == (str(selected_q.state_root), selected_q.native_config_sha256, bundle.name)
        events.append("published")

    monkeypatch.setattr(checker, "retirement_findings", check_workspace)
    monkeypatch.setattr(checker, "publish_qualified_aggregate", publish)

    def check_disposition(root, revision):
        assert root == checker.ROOT and revision == identity[2]
        events.append("disposition")
        return disposition_exit

    monkeypatch.setattr(checker, "check_migration_dispositions", check_disposition)
    result = checker.main(
        evidence, expected_build_receipt_sha256=receipt_sha,
        release_cli_build=cli, expected_cli_build_receipt_sha256=cli_sha,
        shipped_bundle=bundle, installed_readback=readback, trust_root=trust,
        installed_q_approval_request=request,
        proof_checkout=paired_checkout, publish_aggregate=True,
    )
    if disposition_exit:
        assert result == 1 and events == ["source", "disposition"]
        return
    if installed_change:
        assert result == 2
        assert events == ["source", "disposition", "bundle"]
        message = (
            "signed install readback changed" if installed_change == "readback" else "selected binary SHA-256 changed"
        )
        assert message in capsys.readouterr().err
        return
    assert result == (1 if residue else 0)
    assert events == ["source", "disposition", "bundle", "workspace"] + ([] if residue else ["published"])


@pytest.mark.parametrize("changed", [None, "config", "root", "manifest"])
@pytest.mark.parametrize("explicit_aggregate_flag", [False, True])
def test_release_gate_binds_operational_target_to_signed_readback(
    shipped_release, monkeypatch, capsys, changed, explicit_aggregate_flag,
):
    checker, evidence, binary, _, receipt_sha, cli, _, cli_sha, bundle, readback, trust, request, selected_q = shipped_release
    signed = (str(selected_q.state_root), selected_q.native_config_sha256, bundle.name)
    installation = {
        None: signed,
        "config": (signed[0], "0" * 64, signed[2]),
        "root": ("/var/lib/quanta/isolated", signed[1], signed[2]),
        "manifest": (signed[0], signed[1], "6" * 64),
    }[changed]

    def admit(_root, _checkout, *, require_aggregate):
        assert require_aggregate
        return (checker.file_sha256(binary), "b" * 40, "d" * 40), installation

    monkeypatch.setattr(checker, "proof_bound_candidate", admit)
    monkeypatch.setattr(checker, "require_release_source", lambda *_: None)
    monkeypatch.setattr(checker, "verify_paired_product_bundle", lambda *_: None)
    monkeypatch.setattr(checker, "retirement_findings", lambda *_: [])
    monkeypatch.setattr(checker, "check_migration_dispositions", lambda root, revision: 0)
    result = checker.main(
        evidence, expected_build_receipt_sha256=receipt_sha,
        release_cli_build=cli, expected_cli_build_receipt_sha256=cli_sha,
        shipped_bundle=bundle, installed_readback=readback, trust_root=trust,
        installed_q_approval_request=request,
        proof_checkout=checker.ROOT / "s", require_aggregate=explicit_aggregate_flag,
    )
    assert result == (0 if installation == signed else 2)
    if installation != signed:
        assert "operational proofs differ from the signed installation target" in capsys.readouterr().err


@pytest.mark.parametrize(("daemon_pin", "receipt_pin", "error"), [
    (None, "a" * 64, "requires expected daemon and build receipt"),
    ("a" * 64, None, "requires expected daemon and build receipt"),
    ("a" * 64, "invalid", "build receipt SHA-256 must be 64 lowercase hex characters"),
])
def test_release_requires_independent_pins(release_build, daemon_pin, receipt_pin, error, capsys):
    checker, evidence, _, _ = release_build
    assert checker.main(evidence, daemon_pin, expected_build_receipt_sha256=receipt_pin) == 2
    assert error in capsys.readouterr().err


def test_build_receipt_digest_requires_release_evidence(checkout, capsys):
    checker, _ = checkout
    assert checker.main(expected_build_receipt_sha256="a" * 64) == 2
    assert "requires --release-build" in capsys.readouterr().err


@pytest.mark.parametrize(("fault", "expected"), [
    (None, 0),
    ("forbidden_dependency", 1),
    ("failure_after_terminal", 2),
    ("missing_daemon_artifact", 2),
    ("compiler_failed", 2),
    ("wrong_installed_digest", 2),
    ("staged_symlink", 2),
    ("existing_verified", 0),
    ("wrong_verified_bytes", 2),
])
def test_native_release_recipe_checks_its_own_compiler_stream(
    release_build, monkeypatch, capsys, fault, expected
):
    checker, _, binary, events = release_build
    shared = Path(events[0]["executable"])
    shared.write_bytes(b"previous build")
    shared.chmod(0o700)
    target = shared.parent.parent
    expected_digest = "0" * 64 if fault == "wrong_installed_digest" else checker.file_sha256(binary)
    verified = target / "verified-native" / expected_digest / checker.DAEMON_BINARY
    previous_inode = None
    if fault in ("existing_verified", "wrong_verified_bytes"):
        verified.parent.mkdir(parents=True)
        verified.write_bytes(binary.read_bytes() if fault == "existing_verified" else b"wrong")
        verified.chmod(0o700)
        previous_inode = verified.stat().st_ino
    if fault == "forbidden_dependency":
        events.insert(0, {"reason": "compiler-artifact", "package_id": TANTIVY_PACKAGE_ID})
    if fault == "missing_daemon_artifact":
        del events[0]
    if fault == "compiler_failed":
        events[:] = [
            {"reason": "compiler-message", "message": {"rendered": "error: native fixture\n"}},
            {"reason": "build-finished", "success": False},
        ]

    def run(command, *, cwd, stdout, check, env):
        assert cwd == checker.ROOT
        assert env["QUANTA_INDEX_RESOURCE_ADMISSION"] == "1"
        output = Path(command[command.index("--target-dir") + 1])
        assert output.parent == target
        assert command == [
            str(cwd / "scripts/cargow"), "--lane", checker.NATIVE_RELEASE_LANE,
            "build", "-p", checker.WORKSPACE_PACKAGE, "--bin", checker.DAEMON_BINARY,
            "--no-default-features", "--locked", "--offline", "--release",
            "--config", f'build.build-dir={json.dumps(str(cwd / "build-cache"))}',
            "--target-dir", str(output), "--message-format=json-render-diagnostics",
        ]
        staged = output / "release" / checker.DAEMON_BINARY
        staged.parent.mkdir()
        if fault == "staged_symlink":
            staged.symlink_to(shared)
        else:
            staged.write_bytes(binary.read_bytes())
            staged.chmod(0o700)
        for event in events:
            if "executable" in event:
                event["executable"] = str(staged)
        payload = "".join(json.dumps(row) + "\n" for row in events)
        stdout.write(payload.encode())
        assert check is False
        exit_code = 101 if fault in ("failure_after_terminal", "compiler_failed") else 0
        return checker.subprocess.CompletedProcess(command, exit_code)

    monkeypatch.setattr(checker.subprocess, "run", run)
    assert checker.main(expected_daemon_sha256=expected_digest, build_native_daemon=True) == expected
    result = capsys.readouterr()
    if expected == 0:
        assert checker.file_sha256(binary) in result.out
        assert str(verified) in result.out
        assert verified.read_bytes() == binary.read_bytes()
    else:
        assert not result.out
        if fault == "wrong_verified_bytes":
            assert verified.read_bytes() == b"wrong"
        else:
            assert not verified.exists()
    assert shared.read_bytes() == b"previous build"
    if previous_inode is not None:
        assert verified.stat().st_ino == previous_inode
    assert not list(target.glob(".tantivy-release-*"))
    if expected == 1:
        assert "Release compiler artifact: tantivy-fst" in result.err
    if fault == "compiler_failed":
        assert "error: native fixture" in result.err
