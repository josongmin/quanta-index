"""Tests for tools/ci/timing/summarize_cargo_timings.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "timing" / "summarize_cargo_timings.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("summarize_cargo_timings", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["summarize_cargo_timings"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def sample_html() -> str:
    return textwrap.dedent(
        """
        <table class="my-table summary-table">
          <tr><td>Profile:</td><td>dev</td></tr>
          <tr><td>Fresh units:</td><td>0</td></tr>
          <tr><td>Dirty units:</td><td>4</td></tr>
          <tr><td>Max concurrency:</td><td>16 (jobs=16 ncpu=16)</td></tr>
          <tr><td>Total time:</td><td>12.3s</td></tr>
        </table>
        <script>
        const UNIT_DATA = [
          {"name": "tantivy", "target": "", "start": 0.17, "duration": 9.83},
          {"name": "quanta-index-contract", "target": " test \\"lex_property_roundtrip\\" (test)", "start": 1.0, "duration": 7.25},
          {"name": "quanta-index-contract", "target": "", "start": 2.0, "duration": 3.75},
          {"name": "quanta-index-searchctl", "target": " bin \\"quanta-index-searchctl\\"", "start": 3.0, "duration": 4.5}
        ];
        const CONCURRENCY_DATA = [];
        </script>
        """
    )


def test_parse_summary_fields_extracts_key_numbers():
    summary = MODULE.parse_summary_fields(sample_html())
    assert summary["profile"] == "dev"
    assert summary["dirty_units"] == "4"
    assert summary["total_time"] == "12.3s"
    assert summary["max_concurrency"] == "16 (jobs=16 ncpu=16)"


def test_parse_units_reads_unit_data():
    units = MODULE.parse_units(sample_html())
    assert [unit.name for unit in units] == [
        "tantivy",
        "quanta-index-contract",
        "quanta-index-contract",
        "quanta-index-searchctl",
    ]
    assert units[1].duration == 7.25
    assert 'lex_property_roundtrip' in units[1].target


def test_aggregate_repo_crates_sums_matching_crates():
    crates = MODULE.aggregate_repo_crates(
        MODULE.parse_units(sample_html()),
        "quanta-index-",
        limit=5,
    )
    assert crates == [
        {
            "name": "quanta-index-contract",
            "duration": 11.0,
            "units": 2,
        },
        {
            "name": "quanta-index-searchctl",
            "duration": 4.5,
            "units": 1,
        },
    ]


def test_render_text_includes_top_sections():
    summary = MODULE.parse_summary_fields(sample_html())
    units = MODULE.top_units(MODULE.parse_units(sample_html()), limit=2)
    crates = MODULE.aggregate_repo_crates(
        MODULE.parse_units(sample_html()),
        "quanta-index-",
        limit=2,
    )
    rendered = MODULE.render_text(summary, units, crates)
    assert "summary" in rendered
    assert "top_units" in rendered
    assert "top_repo_crates" in rendered
    assert "quanta-index-contract" in rendered
