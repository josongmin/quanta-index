"""Exact bootstrap parity and bounded reuse, independent of evidence admission."""

import math

import pytest

from tools.benchmark.retrieval import evaluator as ev


@pytest.fixture(autouse=True)
def fresh_cache(monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    ev._bootstrap_bounds.cache_clear()
    yield
    ev._bootstrap_bounds.cache_clear()


def test_bootstrap_matches_frozen_preoptimization_golden():
    # Captured from a66eb6b89f77, 10,000 seeded draws, before caching.
    deltas = [0.25, -0.25, 0.5, -0.75]
    strata = [("T1", "a"), ("T2", "a"), ("T3", "b"), ("T4", "b")]
    expected = {
        "lower_95": -0.5, "upper_95": 0.375, "mean": -0.0625,
        "method": "paired_stratified_bootstrap_percentile_v1", "resamples": 10000,
        "sample_count": 4, "strata": {"a": 2, "b": 2},
        "seed_sha256": "fcf1aa0d64a7f777b198bf597449d04e1494a5659827d55edb2f1c47140e6205",
    }
    assert ev.mean_ci(deltas, strata) == expected
    assert ev.mean_ci(deltas[::-1], strata[::-1]) == expected
    assert ev._bootstrap_bounds.cache_info().hits == 1


def test_constant_strata_need_no_random_draws(monkeypatch):
    def refuse_random(*args):
        raise AssertionError("constant-stratum resamples have only one possible mean")

    monkeypatch.setattr(ev.random, "Random", refuse_random)
    result = ev.mean_ci([1.0, 1.0], [("T1", "a"), ("T2", "a")])
    assert result["lower_95"] == result["upper_95"] == result["mean"] == 1.0
    assert result["resamples"] == 10000
    assert result["seed_sha256"] == "5f68f123e873036ece00e61ca44adb9289d1f3e38d970bdc4f778b494c12b6b2"


def test_cached_bounds_cannot_be_mutated_through_result():
    strata = [("T1", "a"), ("T2", "a")]
    result = ev.mean_ci([1.0, 1.0], strata)
    result["lower_95"] = -999
    result["strata"]["a"] = 999
    again = ev.mean_ci([1.0, 1.0], strata)
    assert again["lower_95"] == 1.0
    assert again["strata"] == {"a": 2}


def test_cache_key_preserves_signed_zero_and_all_pair_identities():
    strata = [("T1", "a"), ("T2", "a")]
    outputs = [
        ev.mean_ci([0.0, -0.0], strata),
        ev.mean_ci([0.0, 0.0], strata),
        ev.mean_ci([0.0, -0.0], [("other", "a"), ("T2", "a")]),
        ev.mean_ci([0.0, -0.0], [("T1", "b"), ("T2", "a")]),
        ev.mean_ci([1.0, -0.0], strata),
    ]
    assert len({row["seed_sha256"] for row in outputs}) == 5
    assert ev._bootstrap_bounds.cache_info().misses == 5
    assert outputs[0]["seed_sha256"] == "b5e0ac35c513c29224d35710bab163b8f6e07a4b7e1081c34a2381444ddeedbf"


def test_cache_hit_never_bypasses_validation_or_sample_threshold(monkeypatch):
    strata = [("T1", "a"), ("T2", "a")]
    ev.mean_ci([1.0, 1.0], strata)
    with pytest.raises(ValueError, match="unique"):
        ev.mean_ci([1.0, 1.0], [("T1", "a"), ("T1", "a")])
    with pytest.raises(ValueError, match="finite"):
        ev.mean_ci([math.nan, 1.0], strata)
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 3)
    assert ev.mean_ci([1.0, 1.0], strata)["reason"] == "insufficient_sample"


def test_large_inputs_bypass_cache_and_entry_count_is_bounded():
    for number in range(40):
        ev.mean_ci([float(number)] * 2, [("T1", "a"), ("T2", "a")])
    info = ev._bootstrap_bounds.cache_info()
    assert info.currsize == info.maxsize == 32
    result = ev.mean_ci([1.0, 1.0], [("x" * 65536, "a"), ("T2", "a")])
    assert result["lower_95"] == 1.0
    assert ev._bootstrap_bounds.cache_info() == info


def test_constant_strata_preserve_original_grouped_float_addition_order():
    # Modern Python's compensated sum is not the original bootstrap's ordered
    # float accumulator. Preserve the latter even for degenerate resamples.
    result = ev.mean_ci([1e16, 1.0, -1e16], [("A", "a"), ("B", "b"), ("C", "c")])
    assert result["mean"] == sum([1e16, 1.0, -1e16]) / 3
    assert result["lower_95"] == result["upper_95"] == 0.0


def test_constant_strata_preserve_percentile_interpolation_rounding():
    # Fixed preoptimization oracle: even equal endpoints can round by one ULP.
    result = ev.mean_ci([1 / 61] * 20, [(str(i), "same") for i in range(20)])
    assert result["seed_sha256"] == "1417a52d700ce2114848ef06f0c3701cc198bdd6286e15ca2dd01d4ff70de3ec"
    assert result["lower_95"] == 0.01639344262295082
    assert result["upper_95"] == 0.016393442622950824
