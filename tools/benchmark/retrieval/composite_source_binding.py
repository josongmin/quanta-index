"""Bind one composite Git source to twelve canonical upstream release views.

This is source custody only. Existing Native inventory and pool replay remains
the product authority; a successful binding does not qualify query ranking.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

from tools.benchmark.retrieval import evaluator as ev


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
) -> dict:
    # Release validation and JSON contracts are the existing canonical tools.
    import sys

    sys.path.insert(0, str(Path(ev.__file__).resolve().parents[1]))
    import corpus_release

    universe_bytes, universe = ev.read_json_with_bytes(universe_path)
    binding_bytes, binding = ev.read_json_with_bytes(binding_path)
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
    ev.require(
        binding["family_id"] == family_id
        and binding["universe_sha256"] == hashlib.sha256(universe_bytes).hexdigest()
        and capture.get("source_universe_sha256") == binding["universe_sha256"]
        and capture.get("source_binding_sha256") == hashlib.sha256(binding_bytes).hexdigest()
        and binding["file_universe_digest"] == case["file_universe_digest"]
        and binding["file_count"] == case["file_count"]
        and binding["repository_count"] == 12
        and suite["file_universe_digest"]
        == pack["file_universe_digest"]
        == case["file_universe_digest"]
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
        and "sha256:" + ev.universe_digest(ordered) == case["file_universe_digest"]
        and len(expected) == case["file_count"]
        and source.tracked == set(expected),
        "composite Git file inventory differs",
    )
    base = corpus_release.validate(base_release)
    ev.require(base["digest"] == universe["base_release_digest"], "base release differs")
    base_repositories = [row["recipe"]["name"] for row in base["repositories"]]
    ev.require(
        len(base_repositories) == 12
        and len(set(base_repositories)) == 12
        and set(base_repositories) == set(repositories),
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
    for document in sorted(target_releases.glob("*/release.json")):
        raw = ev.read_json(document)
        if raw.get("digest") == target_digest:
            ev.require(target_digest not in releases, "duplicate target release")
            target = corpus_release.validate(document.parent)
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
