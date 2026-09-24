"""Counterexamples for the registered outcome-enum lint."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "lint" / "check-semantic-outcomes.py"
SPEC = importlib.util.spec_from_file_location("check_semantic_outcomes", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def scan(source: str):
    text = textwrap.dedent(source).encode()
    return MODULE.findings_for_source(
        text, Path("fixture.rs"), MODULE.load_policy(MODULE.POLICY_PATH), MODULE.rust_parser()
    )[0]


def test_negative_variant_cannot_be_rewritten_to_positive():
    findings = scan(
        """
        fn project(status: StructuralReadiness) -> StructuralReadiness {
            match status {
                StructuralReadiness::InvalidRequest(_reason) => StructuralReadiness::Ready,
                other => other,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-01"]


def test_cross_family_negative_cannot_claim_exact_exhaustion():
    findings = scan(
        """
        fn project(status: DenseAdmissionOutcomeV1) -> ExecutionOutcomeV2 {
            match status {
                DenseAdmissionOutcomeV1::Capped => ExecutionOutcomeV2::ExactExhausted,
                _ => ExecutionOutcomeV2::CappedUnknown { cap: 1 },
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-01"]


def test_self_variant_is_recognized_in_impl():
    findings = scan(
        """
        impl StructuralReadiness {
            fn project(self) -> Self {
                match self {
                    Self::GenerationNotReady => Self::Ready,
                    other => other,
                }
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-01"]


@pytest.mark.parametrize(
    "value",
    ["Ok(())", "Ok(None)", "Ok(Vec::new())", "None", "StructuralError::GenerationNotReady"],
)
def test_structural_failure_cannot_become_empty_or_wrong_outcome(value: str):
    findings = scan(
        f"""
        fn project(status: StructuralReadiness) {{
            match status {{
                StructuralReadiness::GenerationNotReady => {value},
                other => other,
            }}
        }}
        """
    )
    assert [finding.rule for finding in findings] == ["SO-03"]


def test_structural_reason_must_reach_the_typed_error():
    findings = scan(
        """
        fn project(status: StructuralReadiness) {
            match status {
                StructuralReadiness::InvalidRequest(message) =>
                    Err(StructuralError::InvalidRequest("unknown".to_owned())),
                StructuralReadiness::ProducerExecution(_) =>
                    Err(StructuralError::ProducerExecution("unknown".to_owned())),
                other => other,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-04", "SO-04"]


def test_structural_reason_preserved_in_exact_error_is_allowed():
    findings = scan(
        """
        fn project(status: StructuralReadiness) {
            match status {
                StructuralReadiness::InvalidRequest(message) =>
                    return Err(StructuralError::InvalidRequest(message.into())),
                StructuralReadiness::ProducerExecution(message) =>
                    return Err(StructuralError::ProducerExecution(message.into())),
                other => other,
            }
        }
        """
    )
    assert findings == []


def test_shadowed_reason_does_not_count_as_preserved():
    findings = scan(
        """
        fn project(status: StructuralReadiness) {
            match status {
                StructuralReadiness::InvalidRequest(message) => {
                    let message = "unknown";
                    Err(StructuralError::InvalidRequest(message.into()))
                }
                other => other,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-04"]


def test_wildcard_cannot_claim_success():
    findings = scan(
        """
        fn project(status: StructuralReadiness) -> StructuralReadiness {
            match status {
                StructuralReadiness::Ready => StructuralReadiness::Ready,
                _ => StructuralReadiness::Ready,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-02"]


def test_qualified_enum_pattern_governs_wildcard():
    findings = scan(
        """
        fn project(status: quanta::StructuralReadiness) {
            match status {
                quanta::StructuralReadiness::Ready => true,
                _ => true,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-02"]


def test_wildcard_typed_failure_is_allowed():
    findings = scan(
        """
        fn project(status: StructuralReadiness) -> Result<(), Error> {
            match status {
                StructuralReadiness::Ready => Ok(()),
                _ => Err(Error::NotReady),
            }
        }
        """
    )
    assert findings == []


def test_wildcard_negative_variant_is_allowed():
    findings = scan(
        """
        fn project(status: StructuralReadiness) -> StructuralReadiness {
            match status {
                StructuralReadiness::Ready => StructuralReadiness::Ready,
                _ => StructuralReadiness::GenerationNotReady,
            }
        }
        """
    )
    assert findings == []


def test_wildcard_none_is_not_an_explicit_failure():
    findings = scan(
        """
        fn project(status: StructuralReadiness) -> Option<bool> {
            match status {
                StructuralReadiness::Ready => Some(true),
                _ => None,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-02"]


def test_bound_catchall_cannot_claim_success():
    findings = scan(
        """
        fn project(status: StructuralReadiness) {
            match status {
                StructuralReadiness::Ready => StructuralReadiness::Ready,
                other => StructuralReadiness::Ready,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-02"]


def test_wildcard_early_success_is_not_hidden_by_terminal_error():
    findings = scan(
        """
        fn project(status: StructuralReadiness) {
            match status {
                StructuralReadiness::Ready => Ok(()),
                _ => { if maybe() { return Ok(()); } Err(Error::NotReady) },
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-02"]


def test_wrapped_pattern_does_not_govern_plain_wildcard():
    findings = scan(
        """
        fn project(status: Option<StructuralReadiness>) -> bool {
            match status {
                Some(StructuralReadiness::Ready) => true,
                _ => false,
            }
        }
        """
    )
    assert findings == []


def test_test_only_code_is_excluded():
    findings = scan(
        """
        #[cfg(test)]
        mod tests {
            fn bad(status: StructuralReadiness) -> bool {
                match status {
                    StructuralReadiness::Ready => true,
                    _ => true,
                }
            }
        }
        """
    )
    assert findings == []


def test_target_parse_error_blocks_scan():
    with pytest.raises(ValueError, match="parse error overlaps"):
        scan(
            """
            fn project(status: StructuralReadiness) {
                match status {
                    StructuralReadiness::Ready => @@@,
                    _ => StructuralReadiness::Ready,
                }
            }
            """
        )


def test_self_target_parse_error_blocks_scan():
    with pytest.raises(ValueError, match="parse error overlaps"):
        scan(
            """
            impl StructuralReadiness {
                fn bad(self) {
                    match self {
                        Self::Ready => @@@,
                        _ => Self::GenerationNotReady,
                    }
                }
            }
            """
        )


def test_unrelated_parser_error_does_not_hide_target():
    findings = scan(
        """
        fn unrelated() { let x = @@@; }
        fn project(status: StructuralReadiness) {
            match status {
                StructuralReadiness::Ready => true,
                _ => true,
            }
        }
        """
    )
    assert [finding.rule for finding in findings] == ["SO-02"]


def test_policy_rejects_duplicate_variant(tmp_path: Path):
    policy = tmp_path / "policy.json"
    policy.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "enum_families": [
                    {
                        "type": "Status",
                        "negative_variants": ["Failed", "Failed"],
                        "positive_variants": ["Ready"],
                        "neutral_variants": [],
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="invalid polarity"):
        MODULE.load_policy(policy)


def test_policy_rejects_partial_error_projection(tmp_path: Path):
    policy = tmp_path / "policy.json"
    policy.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "enum_families": [
                    {
                        "type": "Status",
                        "negative_variants": ["Failed", "Unavailable"],
                        "positive_variants": ["Ready"],
                        "neutral_variants": [],
                        "error_projection": {"Failed": "Error::Failed"},
                        "preserve_payload": [],
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="invalid polarity"):
        MODULE.load_policy(policy)


def test_new_enum_variant_requires_policy_classification(tmp_path: Path):
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    source = tmp_path / "crates/toy/src/lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub enum Status { Ready, Failed, NewVariant }\n", encoding="utf-8")
    family = MODULE.EnumFamily("Status", frozenset({"Failed"}), frozenset({"Ready"}), frozenset())
    with pytest.raises(ValueError, match="does not cover exact enum variants"):
        MODULE.audit_repository(tmp_path, {"Status": family})


def test_repository_registered_enums_are_covered():
    assert MODULE.audit_repository(MODULE.ROOT, MODULE.load_policy(MODULE.POLICY_PATH)) == []
