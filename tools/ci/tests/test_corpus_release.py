"""External corpus views: real Git objects, complete inventories and refusal."""

from __future__ import annotations

import copy
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import corpus_release as corpus
from evidence import EvidenceError


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args])


@pytest.fixture(scope="session")
def source_seed(tmp_path_factory):
    """Build real Git objects once; tests only mutate independent copies."""
    tmp_path = tmp_path_factory.mktemp("corpus-source-seed")
    checkouts = tmp_path / "checkouts"
    root = checkouts / "fixture"
    root.mkdir(parents=True)
    git(root, "init", "-q")
    git(root, "config", "user.name", "Fixture")
    git(root, "config", "user.email", "fixture@localhost")
    files = {
        "src/main.rs": b"fn main() {}\n",
        "src/other.py": b"print('mixed-language')\n",
        "README.md": b"Developer documentation\n",
        "config.toml": b"enabled = true\n",
        "LICENSE": b"Fixture license source, not approval\n",
        "vendor/a.rs": b"excluded vendor code\n",
        "build/generated.rs": b"excluded build output\n",
        "generated/b.rs": b"excluded generated code\n",
        "syntax-error.rs": b"{}}\n",
        "empty.rs": b"",
        "binary.rs": b"hello\0world",
        "encoding.rs": b"\xff",
        "exotic.rs": "a\u2028b".encode(),
        "asset.rs": b"version https://git-lfs.github.com/spec/v1\noid sha256:"
        + b"a" * 64
        + b"\nsize 900\n",
    }
    for name, data in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    (root / "alias.rs").symlink_to("src/main.rs")
    (root / "src/other.py").chmod(0o755)
    git(root, "add", ".")
    git(root, "commit", "-qm", "fixed source")
    revision = git(root, "rev-parse", "HEAD").decode().strip()
    spec = {
        "source_revision": "declared recipe provenance, not qualification",
        "repositories": [
            {
                "name": "fixture",
                "language": "rust",
                "url": "https://example.invalid/fixture.git",
                "revision": revision,
                "benchmark_root": "src",
                "upstream_semble_benchmark_overlap": False,
            }
        ],
    }
    spec_path = tmp_path / "spec.json"
    spec_path.write_text(json.dumps(spec))
    return checkouts, root, spec_path, spec


@pytest.fixture
def source(source_seed, tmp_path):
    checkouts, _, spec_path, spec = source_seed
    target = tmp_path / "checkouts"
    # Copy the object database too: clones/hardlinks can couple corruption tests.
    shutil.copytree(checkouts, target, symlinks=True)
    recipe = tmp_path / "spec.json"
    shutil.copy2(spec_path, recipe)
    return target, target / "fixture", recipe, copy.deepcopy(spec)


@pytest.fixture(scope="session")
def release_seed(source_seed, tmp_path_factory):
    """Reusable producer input for consumer tests, never a cached verdict."""
    target = tmp_path_factory.mktemp("corpus-release-seed") / "release"
    corpus.create(source_seed[2], source_seed[0], target)
    return target


def release(source, tmp_path):
    target = tmp_path / "release"
    document = corpus.create(source[2], source[0], target)
    return target, document


def test_complete_git_inventory_two_views_and_portable_replay(source, tmp_path):
    target, document = release(source, tmp_path)
    repository = document["repositories"][0]
    inventory = {row["path"]: row for row in repository["tracked_inventory"]}
    assert set(inventory) == set(git(source[1], "ls-files").decode().splitlines())
    assert inventory["alias.rs"]["view_exclusions"]["code_only"] == "symlink"
    assert inventory["asset.rs"]["view_exclusions"]["developer_search"] == "git_lfs_pointer"
    assert (
        inventory["vendor/a.rs"]["view_exclusions"]["code_only"] == "generated_or_vendor_component"
    )
    assert (
        inventory["build/generated.rs"]["view_exclusions"]["code_only"]
        == "generated_or_vendor_component"
    )
    assert inventory["syntax-error.rs"]["view_exclusions"]["code_only"] == "no_alphanumeric_token"
    assert inventory["src/main.rs"]["view_exclusions"]["code_only"] is None
    assert inventory["encoding.rs"]["view_exclusions"]["developer_search"] == "non_utf8"
    assert inventory["README.md"]["view_exclusions"] == {
        "code_only": "non_code_extension",
        "developer_search": None,
    }
    code = json.loads((target / repository["views"]["code_only"]["manifest"]).read_text())
    developer = json.loads(
        (target / repository["views"]["developer_search"]["manifest"]).read_text()
    )
    assert [row["path"] for row in code["files"]] == ["src/main.rs", "src/other.py"]
    assert [row["path"] for row in developer["files"]] == [
        "LICENSE",
        "README.md",
        "config.toml",
        "src/main.rs",
        "src/other.py",
    ]
    assert code["repository_commit"] == source[3]["repositories"][0]["revision"]
    assert (target / "views/fixture/code_only/src/other.py").stat().st_mode & 0o111
    assert repository["license_sources"][0]["approval"] == "not_attested"
    assert document["status"] == "frozen_not_admitted"
    (source[1] / "src/main.rs").write_bytes(b"original source mutated")
    assert corpus.validate(target) == document
    second = tmp_path / "second"
    with pytest.raises(EvidenceError, match="clean state differs"):
        corpus.create(source[2], source[0], second)
    assert not second.exists()


@pytest.mark.parametrize(
    "kind", ["view", "blob", "bundle", "manifest", "extra", "mode", "link", "fifo", "inventory"]
)
def test_release_tampering_cannot_be_requalified(source, tmp_path, kind):
    target, document = release(source, tmp_path)
    view = target / "views/fixture/code_only/src/main.rs"
    if kind == "view":
        view.chmod(0o644)
        view.write_bytes(b"tampered")
    elif kind == "blob":
        blob = target / "blobs" / document["repositories"][0]["tracked_inventory"][0]["sha256"]
        blob.write_bytes(b"tampered")
    elif kind == "bundle":
        (target / "bundles/fixture.bundle").write_bytes(b"invalid bundle")
    elif kind == "manifest":
        (target / "manifests/fixture/code_only.json").write_text("{}")
    elif kind == "extra":
        (target / "unlisted").write_bytes(b"extra")
    elif kind == "mode":
        view.chmod(0o555)
    elif kind == "link":
        (target / "linked").symlink_to(source[2])
    elif kind == "fifo":
        os.mkfifo(target / "fifo")
    else:
        document["repositories"][0]["tracked_inventory"].pop()
        body = {key: value for key, value in document.items() if key != "digest"}
        document["digest"] = corpus.digest_bytes(corpus.canonical_json(body).encode())
        (target / "release.json").write_text(corpus.canonical_json(document))
    with pytest.raises((EvidenceError, OSError)):
        corpus.validate(target)


def test_refuses_casefold_parent_collisions(source, tmp_path):
    root = source[1]
    # Create two tree entries without depending on a case-sensitive host FS.
    oid = git(root, "hash-object", "src/main.rs").decode().strip()
    git(root, "update-index", "--add", "--cacheinfo", f"100644,{oid},SRC/alien.rs")
    git(root, "commit", "-qm", "case alias")
    with pytest.raises(EvidenceError, match="case/normalization collision"):
        corpus.tree_rows(root, git(root, "rev-parse", "HEAD").decode().strip())


def test_refuses_duplicate_recipe_wrong_revision_and_overwrite(source, tmp_path):
    spec = copy.deepcopy(source[3])
    spec["repositories"].append(copy.deepcopy(spec["repositories"][0]))
    with pytest.raises(EvidenceError, match="duplicate"):
        corpus.validate_spec(spec)
    with pytest.raises(EvidenceError, match="revision or clean state"):
        corpus.verify_checkout(source[1], "0" * 40)
    target, document = release(source, tmp_path)
    before = (target / "release.json").read_bytes()
    with pytest.raises(EvidenceError, match="never overwrite"):
        corpus.create(source[2], source[0], target)
    assert before == (target / "release.json").read_bytes()
    assert corpus.validate(target) == document


def test_oversize_and_gitlink_are_explicit_not_empty_scores(source, tmp_path):
    root = source[1]
    (root / "large.rs").write_bytes(b"x" * (corpus.POLICY["max_file_bytes"] + 1))
    git(root, "add", "large.rs")
    git(
        root,
        "update-index",
        "--add",
        "--cacheinfo",
        f"160000,{source[3]['repositories'][0]['revision']},nested",
    )
    git(root, "commit", "-qm", "large and gitlink")
    (root / "nested").mkdir()
    spec = copy.deepcopy(source[3])
    spec["repositories"][0]["revision"] = git(root, "rev-parse", "HEAD").decode().strip()
    source[2].write_text(json.dumps(spec))
    target, document = release(source, tmp_path)
    inventory = {row["path"]: row for row in document["repositories"][0]["tracked_inventory"]}
    assert inventory["large.rs"]["view_exclusions"]["code_only"] == "empty_or_oversize"
    assert inventory["large.rs"]["sha256"] is not None
    assert inventory["nested"]["sha256"] is None
    assert inventory["nested"]["view_exclusions"]["code_only"] == "submodule"
    assert corpus.validate(target) == document


def test_repository_inputs_and_release_roots_must_be_external_disjoint(source):
    with pytest.raises(EvidenceError, match="outside"):
        corpus.external(corpus.ROOT / "release")
    with pytest.raises(EvidenceError, match="overlaps"):
        corpus.create(source[2], source[0], source[0] / "release")


def test_public_cli_create_validate_and_dirty_admission(source, tmp_path, monkeypatch, capsys):
    import benchctl

    root = tmp_path / "cli-release"

    def dirty(_):
        raise RuntimeError("worktree is dirty (declared admission fixture)")

    monkeypatch.setattr(benchctl, "require_clean_worktree", dirty)
    assert (
        benchctl.main(
            [
                "corpus",
                "create",
                "--spec",
                str(source[2]),
                "--checkouts",
                str(source[0]),
                "--release",
                str(root),
            ]
        )
        == 2
    )
    assert "worktree is dirty" in capsys.readouterr().err
    assert not root.exists()
    # Declared CLI fixture admission; the independent Git corpus oracle is real.
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda _: None)
    assert (
        benchctl.main(
            [
                "corpus",
                "create",
                "--spec",
                str(source[2]),
                "--checkouts",
                str(source[0]),
                "--release",
                str(root),
            ]
        )
        == 0
    )
    result = json.loads(capsys.readouterr().out)
    assert result["repository_count"] == 1
    assert result["status"] == "frozen_not_admitted"
    assert benchctl.main(["corpus", "validate", "--release", str(root)]) == 0
    assert json.loads(capsys.readouterr().out) == result


def test_source_guard_failure_does_not_publish_or_leave_staging(source, tmp_path):
    root = tmp_path / "refused-release"

    def refused():
        raise EvidenceError("source changed")

    with pytest.raises(EvidenceError, match="source changed"):
        corpus.create(source[2], source[0], root, source_guard=refused)
    assert not root.exists()
    assert not list(tmp_path.glob(".corpus-stage-*"))


def test_git_calls_share_bounded_group_executor(source, monkeypatch):
    observed = []

    def failed(argv, **kwargs):
        observed.append((argv, kwargs))
        raise ValueError("producer timed out; process group killed")

    monkeypatch.setattr(corpus, "execute", failed)
    with pytest.raises(EvidenceError, match="timed out"):
        corpus.git(source[1], "status", "--porcelain")
    assert observed[0][1]["timeout"] == 300
    assert observed[0][1]["cwd"] == source[1]
    assert observed[0][1]["env"]["GIT_ALLOW_PROTOCOL"] == "file"


def test_environment_excludes_service_secrets(monkeypatch):
    monkeypatch.setenv("AWS_SECRET_ACCESS_KEY", "not-a-real-key")
    monkeypatch.setenv("GITHUB_TOKEN", "not-a-real-token")
    monkeypatch.setenv("GIT_CONFIG_COUNT", "1")
    environment = corpus.environment()
    assert not {"AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN", "GIT_CONFIG_COUNT"} & environment.keys()
    assert environment["GIT_CONFIG_GLOBAL"] == os.devnull


@pytest.mark.parametrize(
    "path", [".git/config", ".GIT/config", "../escape", "a/../b", "/absolute", "a\\b"]
)
def test_reserved_and_noncanonical_corpus_paths_refuse(path):
    with pytest.raises(EvidenceError, match="noncanonical"):
        corpus.canonical_path(path)


def test_shallow_history_refuses_before_release_publication(source, tmp_path):
    checkouts, original, recipe, _ = source
    (original / "README.md").write_text("a second commit\n")
    git(original, "add", "README.md")
    git(original, "commit", "-qm", "shallow fixture")
    revision = git(original, "rev-parse", "HEAD").decode().strip()
    shallow_root = tmp_path / "shallow-checkouts"
    shallow_root.mkdir()
    subprocess.run(
        [
            "git",
            "clone",
            "--quiet",
            "--no-local",
            "--depth",
            "1",
            "--",
            str(original),
            str(shallow_root / "fixture"),
        ],
        check=True,
    )
    spec = json.loads(recipe.read_text())
    spec["repositories"][0]["revision"] = revision
    recipe.write_text(json.dumps(spec))
    target = tmp_path / "refused-release"
    with pytest.raises(EvidenceError, match="complete Git history"):
        corpus.create(recipe, shallow_root, target)
    assert not target.exists()
