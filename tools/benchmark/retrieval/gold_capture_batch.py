"""Capture every repository in a release from frozen holdout sampling recipes."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

for path in (Path(__file__).resolve().parents[3], Path(__file__).resolve().parents[1]):
    if str(path) not in sys.path:
        sys.path.insert(0, str(path))

import corpus_binding  # noqa: E402
from evidence import EvidenceError, _read_control_file  # noqa: E402


def capture(
    release: Path,
    other_release: Path,
    sampling: Path,
    output: Path,
) -> dict[str, dict]:
    releases = {}
    for root in (release, other_release):
        document = corpus_binding._json(_read_control_file(root / "release.json"))
        if not isinstance(document, dict) or not isinstance(document.get("digest"), str):
            raise EvidenceError("gold batch release document is malformed")
        digest = document["digest"]
        if digest in releases and releases[digest] != root:
            raise EvidenceError("gold batch release digest has two paths")
        releases[digest] = root
    recipes_dir = sampling / "recipes"
    names = corpus_binding.corpus.regular_tree(recipes_dir)
    if not names or any(
        not name.endswith(".json") or "/" in name or not name[:-5] for name in names
    ):
        raise EvidenceError("gold batch recipe directory has invalid inventory")
    recipes = {name[:-5]: _read_control_file(recipes_dir / name) for name in sorted(names)}
    return corpus_binding.capture_gold_batch(
        release,
        recipes,
        output,
        (_read_control_file(sampling / "split-manifest.json"), releases),
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", required=True, type=Path)
    parser.add_argument("--other-release", required=True, type=Path)
    parser.add_argument("--sampling", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    identities = capture(args.release, args.other_release, args.sampling, args.output)
    print(f"captured {len(identities)} gold capsules at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
