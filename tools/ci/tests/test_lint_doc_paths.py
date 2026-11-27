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


def test_commonmark_angle_destinations_keep_spaces_and_validate_fragments(tmp_path: Path) -> None:
    source = tmp_path / "source.md"
    target = tmp_path / "target with spaces.md"
    source.write_text("# Source\n", encoding="utf-8")
    target.write_text("## Acceptance boundary\n", encoding="utf-8")

    assert LINK_ISSUE(source, "<target with spaces.md#acceptance-boundary>", tmp_path) is None
    assert LINK_ISSUE(source, f"<{target}#acceptance-boundary>", tmp_path) is None
    assert LINK_ISSUE(source, "<target with spaces.md#missing>", tmp_path) == "broken doc anchor"
    assert LINK_ISSUE(source, "<missing.md>", tmp_path) == "broken doc path"
    assert LINK_ISSUE(source, "<../external.md>", tmp_path) == "broken doc path"
    assert LINK_ISSUE(source, "</external/evidence/record.json>", tmp_path) is None


def test_local_source_line_links_require_existing_file_and_line(tmp_path: Path) -> None:
    source = tmp_path / "source.md"
    target = tmp_path / "target.rs"
    literal_colon = tmp_path / "literal:99"
    source.write_text("# Source\n", encoding="utf-8")
    target.write_text("one\ntwo\nthree", encoding="utf-8")
    literal_colon.write_text("literal filename\n", encoding="utf-8")

    assert LINK_ISSUE(source, "target.rs:2", tmp_path) is None
    assert LINK_ISSUE(source, "<target.rs:3>", tmp_path) is None
    assert LINK_ISSUE(source, "target.rs:4", tmp_path) == "broken doc line"
    assert LINK_ISSUE(source, "target.rs:0", tmp_path) == "broken doc path"
    assert LINK_ISSUE(source, "missing.rs:1", tmp_path) == "broken doc path"
    assert LINK_ISSUE(source, "literal:99", tmp_path) is None
    assert LINK_ISSUE(source, "../outside.rs:1", tmp_path) == "broken doc path"
    assert LINK_ISSUE(source, "target.rs:2#heading", tmp_path) == "broken doc path"
