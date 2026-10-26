// SPDX-License-Identifier: Apache-2.0

use core::fmt;

use ciborium::de::{
    from_slice_with_finite_error_and_work_v3, Error, FiniteReadErrorV3, FiniteSemanticReasonV3,
};
use serde::{de, Deserialize, Deserializer};

#[derive(Debug)]
struct CustomRefusal;

struct FormattingMustNotRun;

impl fmt::Display for FormattingMustNotRun {
    #[expect(
        clippy::panic,
        clippy::panic_in_result_fn,
        reason = "this fixed test oracle must fail if the refusal formats a dynamic display value"
    )]
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("native semantic refusal evaluated a dynamic formatter")
    }
}

impl<'de> Deserialize<'de> for CustomRefusal {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(de::Error::custom(FormattingMustNotRun))
    }
}

#[test]
fn finite_custom_refusal_does_not_evaluate_display() {
    let mut scratch = [0; 4];
    let result = from_slice_with_finite_error_and_work_v3::<CustomRefusal, (), _>(
        &[0],
        &mut scratch,
        &mut |_| Ok(()),
    );
    assert!(matches!(
        result,
        Err(Error::Semantic(None, FiniteSemanticReasonV3::CustomRefused))
    ));
}

#[derive(Debug, PartialEq)]
struct OriginalRefusal;

#[test]
fn read_work_refusal_keeps_original_cause() {
    let mut scratch = [0; 4];
    let result = from_slice_with_finite_error_and_work_v3::<u64, OriginalRefusal, _>(
        &[1],
        &mut scratch,
        &mut |bytes| {
            if bytes == 0 {
                Ok(())
            } else {
                Err(OriginalRefusal)
            }
        },
    );
    assert!(matches!(
        result,
        Err(Error::Io(FiniteReadErrorV3::Work(OriginalRefusal)))
    ));
}

#[test]
fn incomplete_input_is_distinct_from_work_refusal() {
    let mut scratch = [0; 4];
    let result =
        from_slice_with_finite_error_and_work_v3::<u64, (), _>(&[0x18], &mut scratch, &mut |_| {
            Ok(())
        });
    assert!(matches!(
        result,
        Err(Error::Io(FiniteReadErrorV3::EndOfInput))
    ));
}

#[test]
fn scratch_refusal_preserves_an_earlier_control_refusal() {
    let result = from_slice_with_finite_error_and_work_v3::<u64, OriginalRefusal, _>(
        &[1],
        &mut [],
        &mut |_| Err(OriginalRefusal),
    );
    assert!(matches!(
        result,
        Err(Error::Io(FiniteReadErrorV3::Work(OriginalRefusal)))
    ));
}

#[test]
fn empty_scratch_is_rejected_before_segment_decode() {
    let result = from_slice_with_finite_error_and_work_v3::<String, (), _>(
        &[0x61, b'a'],
        &mut [],
        &mut |_| Ok(()),
    );
    assert!(matches!(
        result,
        Err(Error::Semantic(_, FiniteSemanticReasonV3::Static(_)))
    ));
}

#[test]
fn valid_wire_has_the_same_value_under_both_policies() {
    let wire = [0x82, 0x01, 0x18, 0x2a];
    let ordinary: Vec<u64> = ciborium::de::from_reader(wire.as_slice()).unwrap();
    let mut scratch = [0; 4];
    let finite = from_slice_with_finite_error_and_work_v3::<Vec<u64>, (), _>(
        &wire,
        &mut scratch,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(finite, ordinary);
}
