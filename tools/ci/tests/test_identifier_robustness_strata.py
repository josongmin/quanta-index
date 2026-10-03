"""Independent text fixtures for source-defined typo diagnostic strata."""

import pytest

from tools.benchmark.retrieval import identifier_robustness_suite as suite
from tools.benchmark.retrieval.identifier_robustness_report import verify_census_against_source
from tools.ci.tests.test_source_oracle_suite import _robustness_baseline


@pytest.mark.parametrize(
    ("original", "query", "relation", "survival"),
    [
        ("FooBar", "FooBaar", "neither", "some"),
        ("FooBar", "FooBbar", "neither", "some"),
        ("Type", "Typ", "query_proper_substring", "none"),
        ("foo_bar", "foo__bar", "neither", "all"),
        ("Go", "Gio", "neither", "no_eligible_components"),
    ],
)
def test_typo_source_strata_fixed_text_cases(original, query, relation, survival):
    assert suite.typo_source_strata(original, query) == {
        "literal_relation": relation,
        "surviving_components": survival,
    }


def test_typo_source_strata_rejects_case_only_and_non_ascii_inputs():
    with pytest.raises(ValueError, match="noisy query equals intended"):
        suite.typo_source_strata("Type", "type")
    with pytest.raises(ValueError, match="ASCII identifiers"):
        suite.typo_source_strata("Týpe", "Type")


def test_source_strata_policy_names_frozen_tokenizer_and_component_threshold():
    assert suite.TYPO_SOURCE_STRATA_POLICY == {
        "component_tokenizer": "camel-snake-v1",
        "min_component_length": 3,
        "component_survival": "casefolded_component_token_identity",
        "tokenizer_scope": "source_oracle_diagnostic_with_inferred_acronym_boundaries",
        "literal_relation": "casefolded_proper_substring",
    }


def test_component_identity_does_not_promote_internal_substring():
    assert "bar" in "FooBbar".casefold()
    assert suite.typo_source_strata("FooBar", "FooBbar")["surviving_components"] == "some"


def test_new_generic_and_paired_censuses_emit_source_replayable_strata(tmp_path):
    repo, baseline = _robustness_baseline(tmp_path)
    generic, generic_census = suite.derive(repo, baseline, seed=7, sample_size=3, no_answer=2)
    paired, paired_census = suite.derive_paired_full(repo, baseline, seed=7)
    for outputs, census, lanes in (
        (generic, generic_census, ("typo",)),
        (
            paired,
            paired_census,
            tuple(
                "typo-" + operation
                for operation in (*suite.TYPO_OPERATIONS, *suite.STRESS_TYPO_LANES)
            ),
        ),
    ):
        for lane in lanes:
            lane_data = census["lanes"][lane]
            assert lane_data["source_strata_policy"] == suite.TYPO_SOURCE_STRATA_POLICY
            admitted = [row for row in lane_data["records"] if row["status"] == "admitted"]
            assert admitted
            assert all(
                row["strata"]["literal_relation"]
                in {"query_proper_substring", "intended_proper_substring", "neither"}
                and row["strata"]["surviving_components"]
                in {"none", "some", "all", "no_eligible_components"}
                for row in admitted
            )
            verify_census_against_source(repo, outputs[lane][0], census, lane)
