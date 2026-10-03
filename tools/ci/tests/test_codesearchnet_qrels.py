"""CodeSearchNet qrel intake preserves upstream judgments without inventing gold."""

from __future__ import annotations

import csv
import hashlib
import io
import json

import pytest

from tools.benchmark.retrieval import codesearchnet_qrels as csn


def _csv(rows: list[tuple[str, str, str, str, str]]) -> bytes:
    output = io.StringIO(newline="")
    writer = csv.writer(output)
    writer.writerow(csn.COLUMNS)
    writer.writerows(rows)
    return output.getvalue().encode()


URL = (
    "https://github.com/example/project/blob/"
    "0123456789abcdef0123456789abcdef01234567/src/module.py#L12-L16"
)


def test_disagreed_judgments_remain_separate_with_fractional_upstream_mean():
    raw = _csv(
        [
            ("Python", "Find a value", URL, "3", "first review"),
            ("Python", "Find a value", URL, "2", "second review"),
            ("Go", "Find a value", URL, "0", ""),
        ]
    )
    aggregation = csn.aggregate_annotations(raw)
    assert "source" not in aggregation
    assert aggregation["counts"] == {
        "raw_annotations": 3,
        "judged_query_language_url": 2,
        "queries": 1,
        "query_language_pairs": 2,
        "disagreed_qrels": 1,
    }
    python_qrel = next(row for row in aggregation["qrels"] if row["language"] == "python")
    assert python_qrel["mean_grade"] == 2.5
    assert python_qrel["grade_histogram"] == {"0": 0, "1": 0, "2": 1, "3": 1}
    assert python_qrel["annotations"] == [
        {
            "record_index": 1,
            "language_original": "Python",
            "query_original": "Find a value",
            "grade": 3,
            "notes": "first review",
        },
        {
            "record_index": 2,
            "language_original": "Python",
            "query_original": "Find a value",
            "grade": 2,
            "notes": "second review",
        },
    ]
    assert python_qrel["source_sha40"] == "0123456789abcdef0123456789abcdef01234567"


@pytest.mark.parametrize("grade", ["NaN", "Infinity", "-1", "4", "2.5", " 2"])
def test_nonfinite_noninteger_and_out_of_range_grades_are_rejected(grade):
    raw = _csv([("Python", "Find a value", URL, grade, "")])
    with pytest.raises(csn.AdmissionError, match="grade"):
        csn.parse_annotations(raw)


@pytest.mark.parametrize(
    "url",
    [
        "https://github.com/example/project/blob/main/src/module.py#L12",
        "https://github.com/example/project/blob/0123456789abcdef0123456789abcdef01234567/src/module.py",
        "https://github.com/example/project/blob/0123456789abcdef0123456789abcdef01234567/../module.py#L12",
        "https://evil.invalid/example/project/blob/0123456789abcdef0123456789abcdef01234567/src/module.py#L12",
        URL.replace("#L12-L16", "#L16-L12"),
    ],
)
def test_unpinned_or_invalid_source_urls_are_rejected(url):
    with pytest.raises(csn.AdmissionError, match="GitHubUrl"):
        csn.parse_annotations(_csv([("Python", "Find a value", url, "3", "")]))


def test_digest_and_csv_columns_are_strict():
    raw = _csv([("Python", "Find a value", URL, "3", "")])
    with pytest.raises(csn.AdmissionError, match="SHA-256"):
        csn.diagnostic_seed(raw)
    with pytest.raises(TypeError):
        csn.diagnostic_seed(raw, expected_sha256=hashlib.sha256(raw).hexdigest())
    with pytest.raises(csn.AdmissionError, match="columns"):
        csn.parse_annotations(raw.replace(b"Notes", b"Reviewer"))
    with pytest.raises(csn.AdmissionError, match="columns"):
        csn.parse_annotations(raw.replace(b"Notes", b"Relevance"))


def test_capture_preserves_existing_output_and_rejects_wrong_source(tmp_path, monkeypatch):
    raw = _csv([("Python", "Find a value", URL, "3", "")])
    source = tmp_path / "annotations.csv"
    source.write_bytes(raw)
    output = tmp_path / "review-seed.json"
    output.write_text("existing receipt")
    monkeypatch.setattr(csn, "UPSTREAM_SHA256", hashlib.sha256(raw).hexdigest())
    with pytest.raises(FileExistsError):
        csn.capture(source, output)
    assert output.read_text() == "existing receipt"
    output.unlink()
    counts = csn.capture(source, output)
    assert counts["raw_annotations"] == 1
    assert json.loads(output.read_text())["qrels"][0]["mean_grade"] == 3.0
    source.write_bytes(raw + b"\n")
    with pytest.raises(csn.AdmissionError, match="SHA-256"):
        csn.capture(source, tmp_path / "second.json")
    assert not (tmp_path / "second.json").exists()


def test_case_normalization_collision_is_rejected_without_dropping_raw_labels():
    raw = _csv(
        [
            ("Python", "Find a value", URL, "3", ""),
            ("Python", "find a value", URL, "2", ""),
        ]
    )
    with pytest.raises(csn.AdmissionError, match="normalization collision"):
        csn.aggregate_annotations(raw)


def test_output_must_be_absolute_and_outside_resolved_checkout(tmp_path, monkeypatch):
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    alias = tmp_path / "checkout-alias"
    alias.symlink_to(checkout, target_is_directory=True)
    monkeypatch.setattr(csn, "TOOL_CHECKOUT", checkout.resolve())
    for output in (checkout / "seed.json", alias / "seed.json"):
        with pytest.raises(csn.AdmissionError, match="absolute|outside"):
            csn._check_output(output)
    with pytest.raises(csn.AdmissionError, match="absolute"):
        csn._check_output(type(tmp_path)("relative.json"))
    assert not (checkout / "seed.json").exists()
