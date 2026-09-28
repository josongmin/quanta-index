"""Local Markdown links must resolve to both a file and its heading."""

from pathlib import Path
from runpy import run_path

LINK_ISSUE = run_path(str(Path(__file__).resolve().parents[1] / "lint" / "lint-doc-paths.py"))[
    "link_issue"
]


def test_local_heading_fragments_are_checked(tmp_path: Path) -> None:
    source = tmp_path / "source.md"
    target = tmp_path / "target.md"
    source.write_text("# Source\n", encoding="utf-8")
    target.write_text("## Serial acceptance boundary\n", encoding="utf-8")

    assert LINK_ISSUE(source, "target.md#serial-acceptance-boundary", tmp_path) is None
    assert LINK_ISSUE(source, "#source", tmp_path) is None
    assert LINK_ISSUE(source, "target.md#remaining-work", tmp_path) == "broken doc anchor"
    assert LINK_ISSUE(source, "missing.md#source", tmp_path) == "broken doc path"
    assert LINK_ISSUE(source, "https://example.com/doc#remote", tmp_path) is None
