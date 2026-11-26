"""Regressions for the Runtime Legacy source-custody gate."""

import hashlib
import importlib.util
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

CHECKER = Path(__file__).resolve().parents[1] / "lint/check-runtime-legacy-migration.py"


@pytest.fixture
def checker(tmp_path, monkeypatch):
    spec = importlib.util.spec_from_file_location("runtime_legacy_migration", CHECKER)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    manifest = tmp_path / "runtime-legacy-migration.toml"
    manifest.write_text(module.MANIFEST.read_text(encoding="utf-8"), encoding="utf-8")
    monkeypatch.setattr(module, "MANIFEST", manifest)
    return module


def test_current_roster_preserves_every_frozen_body(checker, capsys):
    assert checker.main() == 0
    output = capsys.readouterr().out
    assert "43 perf IDs map to current bodies" in output
    assert "4 exact internal oracles are preserved" in output


def test_retirement_rejects_pending_runtime_execution(checker, capsys):
    source = checker.MANIFEST.read_text(encoding="utf-8")
    checker.MANIFEST.write_text(
        source.replace('proof = "VERIFIED"', 'proof = "NOT_RUN"'), encoding="utf-8"
    )
    assert checker.main() == 0
    assert checker.main(require_execution=True) == 1
    errors = capsys.readouterr().err
    assert "current proof group 1: retirement requires VERIFIED execution" in errors
    assert ".rs:" in errors


def test_runtime_verified_label_requires_execution_observations(checker, capsys, monkeypatch):
    roster = checker.tomllib.loads(checker.MANIFEST.read_text(encoding="utf-8"))
    proof = roster["current_proofs"][0]
    proof["proof"] = "VERIFIED"
    proof.pop("observations", None)
    monkeypatch.setattr(checker, "tomllib", SimpleNamespace(loads=lambda _: roster))
    assert checker.main(require_execution=True) == 1
    assert "current proof group 1: VERIFIED disposition requires" in capsys.readouterr().err


@pytest.mark.parametrize(
    ("index", "event_prefix"),
    [
        (0, "quanta-index-searchd-runtime::process_readiness_owner_v1$e2e_perf_chaos::"),
        (3, "quanta-index-searchd-runtime::runtime_fast_suite$sdk_frontdoor::matrix_tests::"),
    ],
)
def test_runtime_observation_accepts_actual_grouped_test_owner(
    checker, monkeypatch, capsys, index, event_prefix
):
    manifest = checker.MANIFEST.read_text(encoding="utf-8")
    parse_toml = checker.tomllib.loads
    roster = parse_toml(manifest)
    proof = roster["current_proofs"][index]
    proof["proof"] = "VERIFIED"
    proof["observations"] = [
        {
            "source": candidate["path"],
            "case": case,
            "source_sha256": hashlib.sha256(
                (checker.ROOT / candidate["path"]).read_bytes()
            ).hexdigest(),
            "run_id": "3739620b-a0ff-451d-897f-e5a32fc146eb",
            "event": event_prefix + case,
            "events_sha256": "a" * 64,
        }
        for candidate in proof["candidates"]
        for case in candidate["tests"]
    ]
    monkeypatch.setattr(
        checker,
        "tomllib",
        SimpleNamespace(loads=lambda raw: roster if raw == manifest else parse_toml(raw)),
    )
    assert checker.main() == 0
    capsys.readouterr()
    proof["observations"][0]["event"] = (
        event_prefix + "wrong_module::" + proof["observations"][0]["case"]
    )
    assert checker.main() == 1
    assert "differs from" in capsys.readouterr().err


def test_runtime_event_owner_uses_committed_launcher(checker, monkeypatch, tmp_path):
    root = tmp_path / "source"
    tests = root / "crates/runtime/tests"
    tests.mkdir(parents=True)
    catalog = root / "tools/ci/test-authority.toml"
    catalog.parent.mkdir(parents=True)
    catalog.write_text(
        '[[integration_targets]]\npath = "crates/runtime/tests/suite.rs"\nowner = "runtime"\n'
        '[[integration_targets]]\npath = "crates/runtime/tests/member.rs"\nowner = "runtime"\ntarget = "suite"\n',
        encoding="utf-8",
    )
    source = tests / "member.rs"
    source.write_text("#[test]\nfn exact() {}\n", encoding="utf-8")
    launcher = tests / "suite.rs"
    launcher.write_text('#[path = "member.rs"]\nmod original;\n', encoding="utf-8")
    git = ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test"]
    for args in (["init", "-q"], ["add", "."], ["commit", "-qm", "candidate"]):
        subprocess.run([*git, *args], cwd=root, check=True)
    revision = subprocess.check_output([*git, "rev-parse", "HEAD"], cwd=root, text=True).strip()
    launcher.write_text('#[path = "member.rs"]\nmod changed;\n', encoding="utf-8")
    monkeypatch.setattr(checker, "ROOT", root)
    committed = checker.runtime_event_matcher(revision)
    assert committed(source, "runtime::suite", "original::exact")
    assert not committed(source, "runtime::suite", "changed::exact")
    assert not committed(source, "runtime::other_suite", "original::exact")


def test_current_public_route_runner_admits_additional_tests(checker, tmp_path, monkeypatch):
    source = checker.CURRENT_PERF.read_text(encoding="utf-8")
    path = tmp_path / "legacy_perf_chaos_routes.rs"
    path.write_text(source + "\n#[test]\nfn added_public_route_fixture() {}\n", encoding="utf-8")
    monkeypatch.setattr(checker, "CURRENT_PERF", path)
    assert checker.main() == 0


def test_current_public_route_runner_still_requires_original_cases(
    checker,
    capsys,
    tmp_path,
    monkeypatch,
):
    required = "regex_false_positive_candidate_is_rejected_by_exact_verify"
    source = checker.CURRENT_PERF.read_text(encoding="utf-8")
    path = tmp_path / "legacy_perf_chaos_routes.rs"
    path.write_text(source.replace(f"fn {required}(", "fn unrelated_route("), encoding="utf-8")
    monkeypatch.setattr(checker, "CURRENT_PERF", path)
    assert checker.main() == 1
    assert f"required public-route ID missing: {required}" in capsys.readouterr().err


def test_same_count_perf_id_replacement_is_rejected(checker, capsys):
    source = checker.MANIFEST.read_text(encoding="utf-8")
    checker.MANIFEST.write_text(
        source.replace(
            '"history_before_executes_and_metrics_are_bounded"',
            '"unrelated_same_count_runtime_test"',
        ),
        encoding="utf-8",
    )
    assert checker.main() == 1
    assert "frozen 43-test census" in capsys.readouterr().err


def test_pending_exact_oracle_cannot_be_declared_retired(checker, capsys):
    source = checker.MANIFEST.read_text(encoding="utf-8")
    checker.MANIFEST.write_text(
        source.replace('disposition = "migrate"', 'disposition = "retire"', 1),
        encoding="utf-8",
    )
    assert checker.main() == 1
    assert "cannot be retired before exact proof" in capsys.readouterr().err


def test_removing_an_exact_oracle_is_rejected(checker, capsys):
    source = checker.MANIFEST.read_text(encoding="utf-8")
    marker = "\n[[exact_oracles]]\n"
    first, rest = source.split(marker, 1)
    _removed, remaining = rest.split(marker, 1)
    checker.MANIFEST.write_text(first + marker + remaining, encoding="utf-8")
    assert checker.main() == 1
    errors = capsys.readouterr().err
    assert "all four exact internal oracles" in errors
    assert "frozen four" in errors


def test_missing_current_candidate_body_is_rejected(checker, capsys):
    source = checker.MANIFEST.read_text(encoding="utf-8")
    checker.MANIFEST.write_text(
        source.replace(
            '"history_before_executes_on_the_pinned_generation"',
            '"history_before_candidate_was_deleted"',
            1,
        ),
        encoding="utf-8",
    )
    assert checker.main() == 1
    assert (
        "has no body-bearing test history_before_candidate_was_deleted" in capsys.readouterr().err
    )


def test_test_attribute_cannot_attach_to_a_later_helper(checker):
    source = """\
#[test]
const REMOVED_TEST_SENTINEL: bool = true;

fn unrelated_helper() {}
"""
    with pytest.raises(ValueError, match="is not attached to a function"):
        checker.source_tests("candidate.rs", source)


def test_unmapped_frozen_test_is_rejected(checker, capsys):
    source = checker.MANIFEST.read_text(encoding="utf-8")
    needle = '  "structural_orphan_chunk_authority_is_refused_before_readiness",\n'
    first = source.find(needle)
    second = source.find(needle, first + len(needle))
    assert first >= 0 and second >= 0
    checker.MANIFEST.write_text(
        source[:second] + source[second + len(needle) :],
        encoding="utf-8",
    )
    assert checker.main() == 1
    assert "current proof mapping differs from the frozen 43-test census" in capsys.readouterr().err
