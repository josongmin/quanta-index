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
    assert LINK_ISSUE(source, "/external/evidence/record.json", tmp_path) is None
    assert LINK_ISSUE(source, str(target), tmp_path) is None
    assert LINK_ISSUE(source, str(tmp_path / "missing.md"), tmp_path) == "broken doc path"
    assert LINK_ISSUE(source, "../external.md", tmp_path) == "broken doc path"


def test_explicit_html_targets_preserve_exact_fragment_identity(tmp_path: Path) -> None:
    source = tmp_path / "source.md"
    target = tmp_path / "target.md"
    source.write_text("# Source\n", encoding="utf-8")
    target.write_text(
        '| Ticket | <a id="w2-02"></a> |\n'
        "<span id='Exact-Identity'></span>\n"
        '<a name="legacy-boundary"></a>\n'
        '<span name="not-an-anchor" data-id="also-not-an-anchor"></span>\n',
        encoding="utf-8",
    )

    for fragment in ("w2-02", "Exact-Identity", "legacy-boundary"):
        assert LINK_ISSUE(source, f"target.md#{fragment}", tmp_path) is None
    for fragment in ("w2-03", "exact-identity", "not-an-anchor", "also-not-an-anchor"):
        assert LINK_ISSUE(source, f"target.md#{fragment}", tmp_path) == "broken doc anchor"
