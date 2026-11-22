"""Bind one composite Git source to twelve canonical upstream release views.

This is source custody only. Existing Native inventory and pool replay remains
the product authority; a successful binding does not qualify query ranking.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

from tools.benchmark.retrieval import evaluator as ev


def _corpus_release():
    benchmark_root = str(Path(__file__).resolve().parents[1])
    if benchmark_root not in sys.path:
        sys.path.insert(0, benchmark_root)
    import corpus_release

    return corpus_release


def validate_release(root: Path) -> dict:
    return _corpus_release().validate(root)


def _require_target_change(base: dict, target: dict) -> None:
    ev.require(
        target["recipe"]["url"] == base["recipe"]["url"],
        "composite source target upstream repository differs from base",
    )
    ev.require(
        target["recipe"]["revision"] != base["recipe"]["revision"],
        "composite source target commit must differ from base",
    )
    ev.require(
        target["views"]["code_only"]["file_universe_digest"]
        != base["views"]["code_only"]["file_universe_digest"],
        "composite source target code-only universe must differ from base",
    )


def _commit_source(root: Path, message: str, git_environment: dict) -> ev.SourceSnapshot:
    environment = {
        **git_environment,
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    }
    commands = (
        ["init", "-q", "-b", "main", "--object-format=sha1", "--template="],
        ["add", "--force", "--all"],
        ["commit", "-qm", message],
        ["rev-parse", "HEAD"],
    )
    git = [
        "git", "-c", "core.autocrlf=false", "-c", "core.filemode=true",
        "-c", "user.name=Quanta Benchmark", "-c", "user.email=benchmark@example.invalid",
    ]
    for command in commands:
        try:
            result = subprocess.run(
                [*git, *command], cwd=root, env=environment, check=True,
                capture_output=True, timeout=300,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise ev.EvidenceError(f"composite source Git {command[0]} failed") from exc
    return ev.SourceSnapshot(root, result.stdout.decode("ascii").strip())


def build(base_release: Path, target_release: Path, output: Path) -> dict:
    """Freeze one changed repository per family from two verified source releases."""
    corpus_release = _corpus_release()
    ev.require(output.is_absolute(), "composite source output must be absolute")
    corpus_release.external(output)
    for root in (base_release, target_release):
        ev.require(
            not output.resolve().is_relative_to(root.resolve()),
            "composite source output must be outside the input releases",
        )
    if output.exists():
        raise FileExistsError(f"composite source destination already exists: {output}")
    base = corpus_release.validate(base_release)
    target = corpus_release.validate(target_release)
    base_rows = {row["recipe"]["name"]: row for row in base["repositories"]}
    target_rows = {row["recipe"]["name"]: row for row in target["repositories"]}
    ev.require(
        len(base_rows) == 12 and base_rows.keys() == target_rows.keys(),
        "composite builds require the same twelve repositories in both releases",
    )
    names = sorted(base_rows)
    for name in names:
        _require_target_change(base_rows[name], target_rows[name])
    output.mkdir(mode=0o700)
    try:
        cases, bindings = [], []
        for changed in names:
            root = output / changed / "source"
            root.mkdir(parents=True)
            files, roster = [], []
            for index, name in enumerate(names):
                namespace = f"r{index:02d}"
                release, document, row = (
                    (target_release, target, target_rows[name]) if name == changed
                    else (base_release, base, base_rows[name])
                )
                view = row["views"]["code_only"]
                roster.append({
                    "repository": name, "namespace": namespace,
                    "release_digest": document["digest"], "commit": row["recipe"]["revision"],
                    "file_universe_digest": view["file_universe_digest"],
                    "file_count": view["file_count"],
                })
                raw, manifest = ev.read_json_with_bytes(release / view["manifest"])
                ev.require(
                    f"sha256:{ev.digest(raw)}" == view["manifest_digest"],
                    "composite source view manifest changed after release validation",
                )
                for file in manifest["files"]:
                    relative = f"{namespace}/{file['path']}"
                    original = release / "views" / name / "code_only" / file["path"]
                    destination = root / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(original, destination)
                    destination.chmod(0o755 if original.stat().st_mode & 0o111 else 0o644)
                    files.append({**file, "path": relative})
            source = _commit_source(root, f"Composite source for {changed}", corpus_release.environment())
            expected, ordered = ev.validate_file_universe(source, files)
            ev.require(source.tracked == set(expected), "composite Git file inventory differs")
            case = {
                "family_id": changed, "target_repository": changed,
                "target_commit": target_rows[changed]["recipe"]["revision"],
                "release_roster": roster, "files": ordered, "file_count": len(ordered),
                "file_universe_digest": f"sha256:{ev.universe_digest(ordered)}",
            }
            cases.append(case)
            bindings.append({
                "kind": "provisional_composite_git_source_binding_v1",
                "status": "SOURCE_ONLY_NOT_FORMAL_EVAL",
                "family_id": changed, "source_commit": source.commit,
                "file_universe_digest": case["file_universe_digest"],
                "file_count": len(ordered), "repository_count": len(roster),
            })
        universe = {
            "kind": "provisional_source_only_twelve_repo_universes_v1",
            "status": "NOT_ADMITTED_NO_QRELS_NO_SDK_RANKS",
            "base_release_digest": base["digest"], "cases": cases,
        }
        encoded = ev.canonical(universe)
        for binding in bindings:
            binding["universe_sha256"] = ev.digest(encoded)
            (output / binding["family_id"] / "source-binding.json").write_bytes(ev.canonical(binding))
        (output / "source-universe.json").write_bytes(encoded)
        return universe
    except BaseException:
        shutil.rmtree(output)
        raise


def verify(
    source: ev.SourceSnapshot,
    suite: dict,
    pack: dict,
    capture: dict,
    universe_path: Path,
    binding_path: Path,
    base_release: Path,
    target_releases: Path,
    family_id: str,
    *,
    release_validator: Callable[[Path], dict] | None = None,
    expected_source_universe: tuple[str, int] | None = None,
    expected_source_binding: tuple[str, int] | None = None,
) -> dict:
    read_release = validate_release if release_validator is None else release_validator

    universe_bytes, universe = ev.read_json_with_bytes(
        universe_path, expected_bytes=expected_source_universe,
    )
    binding_bytes, binding = ev.read_json_with_bytes(
        binding_path, expected_bytes=expected_source_binding,
    )
    ev.require(
        universe.get("kind") == "provisional_source_only_twelve_repo_universes_v1"
        and universe.get("status") == "NOT_ADMITTED_NO_QRELS_NO_SDK_RANKS"
        and binding.get("kind") == "provisional_composite_git_source_binding_v1"
        and binding.get("status") == "SOURCE_ONLY_NOT_FORMAL_EVAL",
        "composite source evidence kind or status differs",
    )
    cases = [case for case in universe["cases"] if case["family_id"] == family_id]
    ev.require(len(cases) == 1, "composite source family is missing or repeated")
    case = cases[0]
    suite_universe_digest = ev.sha(suite["file_universe_digest"], "suite file universe digest")
    ev.require(
        binding["family_id"] == family_id
        and binding["universe_sha256"] == hashlib.sha256(universe_bytes).hexdigest()
        and capture.get("source_universe_sha256") == binding["universe_sha256"]
        and capture.get("source_binding_sha256") == hashlib.sha256(binding_bytes).hexdigest()
        and binding["file_universe_digest"] == case["file_universe_digest"]
        and binding["file_count"] == case["file_count"]
        and binding["repository_count"] == 12
        and pack["file_universe_digest"] == suite_universe_digest
        and case["file_universe_digest"] == f"sha256:{suite_universe_digest}"
        and suite["file_universe"] == case["files"]
        and pack["file_universe"] == case["files"]
        and binding["source_commit"]
        == suite["repository_commit"]
        == pack["repository_commit"]
        == source.commit,
        "composite source/query/capture binding differs",
    )
    roster = case["release_roster"]
    ev.require(len(roster) == 12, "composite source roster must contain twelve repositories")
    repositories = [spec["repository"] for spec in roster]
    namespaces = [spec["namespace"] for spec in roster]
    ev.require(
        len(set(repositories)) == len(set(namespaces)) == 12,
        "composite source roster must contain unique repositories and namespaces",
    )
    expected_namespaces = {
        repository: f"r{index:02d}" for index, repository in enumerate(sorted(repositories))
    }
    ev.require(
        all(spec["namespace"] == expected_namespaces[spec["repository"]] for spec in roster),
        "composite source repository namespace differs from canonical order",
    )
    expected, ordered = ev.validate_file_universe(source, case["files"])
    ev.require(
        ordered == case["files"]
        and ev.universe_digest(ordered) == suite_universe_digest
        and len(expected) == case["file_count"]
        and source.tracked == set(expected),
        "composite Git file inventory differs",
    )
    base = read_release(base_release)
    ev.require(base["digest"] == universe["base_release_digest"], "base release differs")
    base_rows = {row["recipe"]["name"]: row for row in base["repositories"]}
    ev.require(
        len(base["repositories"]) == len(base_rows) == 12
        and set(base_rows) == set(repositories),
        "composite source roster differs from base release repositories",
    )
    releases = {base["digest"]: (base_release, base)}
    target_specs = [spec for spec in roster if spec["repository"] == case["target_repository"]]
    ev.require(len(target_specs) == 1, "composite source target repository is missing or repeated")
    target_spec = target_specs[0]
    target_digest = target_spec["release_digest"]
    ev.require(
        target_digest != base["digest"] and target_spec["commit"] == case["target_commit"],
        "composite source target release or revision differs",
    )
    ev.require(
        all(spec["release_digest"] == base["digest"] for spec in roster if spec is not target_spec),
        "composite source non-target release differs from base",
    )
    direct_release = target_releases / "release.json"
    child_releases = sorted(target_releases.glob("*/release.json"))
    ev.require(
        not (direct_release.exists() and child_releases),
        "target release layout is ambiguous",
    )
    documents = [direct_release] if direct_release.exists() else child_releases
    for document in documents:
        raw = ev.read_json(document)
        if raw.get("digest") == target_digest:
            ev.require(target_digest not in releases, "duplicate target release")
            target = read_release(document.parent)
            ev.require(target["digest"] == target_digest, "target release digest differs")
            releases[target_digest] = (document.parent, target)
    ev.require(target_digest in releases, "target release is absent")
    observed: dict[str, str] = {}
    for spec in roster:
        root, release = releases[spec["release_digest"]]
        matching = [
            row for row in release["repositories"] if row["recipe"]["name"] == spec["repository"]
        ]
        ev.require(len(matching) == 1, "source release repository missing or duplicate")
        row = matching[0]
        if spec is target_spec:
            _require_target_change(base_rows[spec["repository"]], row)
        view = row["views"]["code_only"]
        ev.require(
            row["recipe"]["revision"] == spec["commit"]
            and view["file_universe_digest"] == spec["file_universe_digest"]
            and view["file_count"] == spec["file_count"],
            "source release revision or view differs",
        )
        files = ev.read_json(root / view["manifest"])["files"]
        ev.require(len(files) == spec["file_count"], "source release file count differs")
        for item in files:
            path = f"{spec['namespace']}/{item['path']}"
            ev.require(
                path in expected and expected[path] == item["file_sha256"],
                "source release file differs from frozen universe",
            )
            original = (
                root / "views" / spec["repository"] / "code_only" / item["path"]
            ).read_bytes()
            raw, _lines, digest = source.file(path)
            ev.require(
                raw == original
                and digest == item["file_sha256"]
                and hashlib.sha256(original).hexdigest() == item["file_sha256"],
                "composite Git bytes differ from exact upstream release",
            )
            observed[path] = digest
    ev.require(observed == expected, "composite source omitted or invented a file")
    return {
        "family_id": family_id,
        "source_release_count": 12,
        "source_file_count": len(expected),
        "source_universe_sha256": hashlib.sha256(universe_bytes).hexdigest(),
        "source_binding_sha256": hashlib.sha256(binding_bytes).hexdigest(),
        "source_commit": source.commit,
        "file_universe_digest": case["file_universe_digest"],
        "base_release_digest": base["digest"],
        "target_repository": case["target_repository"],
        "target_commit": target_spec["commit"],
        "target_release_digest": target_digest,
        "qualification": "SOURCE_ONLY_NOT_FORMAL_EVAL",
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base-release", type=Path, required=True)
    parser.add_argument("--target-release", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        build(args.base_release, args.target_release, args.output)
    except (OSError, ValueError) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
