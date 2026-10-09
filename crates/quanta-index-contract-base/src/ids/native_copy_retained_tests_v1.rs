//! Authored owner regressions; mock admission is not Original Source proof.

use super::{
    NativeIdentityCopyErrorV1 as Failure, NativeIdentityCopyRefusalV1 as Refusal, RepoId,
    RevisionId, admit_native_copy_backing_v1, retain_native_copy_attempt_v1,
    try_copy_string_into_slots_with_native_birth_v1 as copy,
};
use core::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy)]
enum Disposition {
    ReturnFailure,
    ReportSuccess,
    Repeat,
    RefuseLate,
}

#[test]
fn real_reserve_cause_survives_later_protocol_and_admission_failures() {
    // Independent std capacity-overflow oracle; no OOM attempt or forged str.
    let expected = String::new()
        .try_reserve_exact(usize::MAX)
        .expect_err("capacity overflow");
    for disposition in [
        Disposition::ReturnFailure,
        Disposition::ReportSuccess,
        Disposition::Repeat,
        Disposition::RefuseLate,
    ] {
        let original = Box::new(83_u8);
        let original_pointer = core::ptr::from_ref(original.as_ref());
        let mut original = Some(original);
        let mut backing = String::new();
        let mut attempted = false;
        let mut failure = None;
        let calls = Cell::new(0_u32);
        assert_eq!(
            retain_native_copy_attempt_v1(&mut backing, &mut attempted, &mut failure, |backing| {
                admit_native_copy_backing_v1(usize::MAX, backing, |bytes, birth| {
                    assert_eq!(bytes, usize::MAX);
                    calls.set(calls.get() + 1);
                    assert!(!birth());
                    match disposition {
                        Disposition::ReturnFailure => Ok(false),
                        Disposition::ReportSuccess => Ok(true),
                        Disposition::Repeat => {
                            assert!(!birth());
                            Ok(false)
                        }
                        Disposition::RefuseLate => Err(original.take().expect("original cause")),
                    }
                })
            }),
            Err(Refusal::OperationRefused)
        );
        assert!(attempted);
        assert_eq!(calls.get(), 1);
        assert_eq!(backing.capacity(), 0);
        let retained = failure.as_ref().expect("complete cause");
        assert_eq!(retained.reserve_failure_v1(), Some(&expected));
        match disposition {
            Disposition::ReturnFailure => {
                assert!(matches!(retained, Failure::NativeAllocationFailed(_)));
                assert!(retained.admission_failure_v1().is_none());
            }
            Disposition::ReportSuccess | Disposition::Repeat => {
                assert!(matches!(
                    retained,
                    Failure::InvalidNativeProducerAfterReserveFailure(_)
                ));
                assert!(retained.admission_failure_v1().is_none());
            }
            Disposition::RefuseLate => {
                assert!(matches!(
                    retained,
                    Failure::AdmissionAfterReserveFailure { .. }
                ));
                assert_eq!(
                    core::ptr::from_ref(
                        retained.admission_failure_v1().expect("original").as_ref()
                    ),
                    original_pointer
                );
            }
        }
        let retained_pointer = core::ptr::from_ref(retained);
        assert_eq!(
            copy(
                "unused",
                &mut backing,
                &mut attempted,
                &mut failure,
                |_, _| { panic!("occupied error slot must not poll") }
            ),
            Err(Refusal::OccupiedOutput)
        );
        assert_eq!(
            core::ptr::from_ref(failure.as_ref().expect("first cause")),
            retained_pointer
        );
        let first = failure.take().expect("pure transfer of first cause");
        assert_eq!(
            copy(
                "unused",
                &mut backing,
                &mut attempted,
                &mut failure,
                |_, _| { panic!("used attempt must not poll") }
            ),
            Err(Refusal::UsedData)
        );
        assert!(failure.is_none());
        assert_eq!(first.reserve_failure_v1(), Some(&expected));
    }
}

struct Funding(Rc<Cell<u32>>);
impl Drop for Funding {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn unit_copy_keeps_born_backing_original_error_and_external_funding() {
    let drops = Rc::new(Cell::new(0));
    let mut funding = None;
    let original = Box::new(89_u8);
    let pointer = core::ptr::from_ref(original.as_ref());
    let mut backing = String::new();
    let mut attempted = false;
    let mut failure = None;
    assert_eq!(
        copy(
            "repo/test",
            &mut backing,
            &mut attempted,
            &mut failure,
            |bytes, birth| {
                assert_eq!(bytes, 9);
                funding = Some(Funding(Rc::clone(&drops)));
                assert!(birth());
                Err::<bool, _>(original)
            }
        ),
        Err(Refusal::OperationRefused)
    );
    assert!(attempted);
    assert!(backing.is_empty());
    assert_eq!(backing.capacity(), 9);
    let retained = failure.as_ref().expect("original error");
    assert_eq!(
        core::ptr::from_ref(retained.admission_failure_v1().expect("admission").as_ref()),
        pointer
    );
    assert!(retained.reserve_failure_v1().is_none());
    assert_eq!(drops.get(), 0);
    let backing_pointer = backing.as_ptr();
    assert_eq!(
        copy(
            "unused",
            &mut backing,
            &mut attempted,
            &mut failure,
            |_, _| { panic!("first failure and born backing must be preserved") }
        ),
        Err(Refusal::OccupiedOutput)
    );
    assert_eq!(backing.as_ptr(), backing_pointer);
    assert_eq!(backing.capacity(), 9);
    drop(backing);
    drop(failure);
    assert_eq!(drops.get(), 0);
    drop(funding);
    assert_eq!(drops.get(), 1);
}

#[test]
fn unit_typed_clones_keep_private_bytes_and_one_shot_state() {
    let repo = RepoId::new("répo/../%").expect("canonical repo");
    let mut backing = String::new();
    let mut output = None;
    let mut attempted = false;
    let mut failure = None;
    repo.try_clone_into_slots_with_native_birth_v1(
        &mut backing,
        &mut output,
        &mut attempted,
        &mut failure,
        |bytes, birth| {
            assert_eq!(bytes, 10);
            Ok::<_, u8>(birth())
        },
    )
    .expect("same sealed clone");
    assert_eq!(output.as_ref(), Some(&repo));
    assert!(attempted);
    assert!(failure.is_none());
    assert_eq!(backing.capacity(), 0);
    assert_ne!(
        output.as_ref().expect("clone").as_str().as_ptr(),
        repo.as_str().as_ptr()
    );
    let copied = output.take().expect("pure output transfer");
    assert_eq!(
        repo.try_clone_into_slots_with_native_birth_v1(
            &mut backing,
            &mut output,
            &mut attempted,
            &mut failure,
            |_, _| panic!("used clone attempt must not poll"),
        ),
        Err(Refusal::UsedData)
    );
    assert_eq!(copied, repo);

    let revision = RevisionId::new("rev/test").expect("canonical revision");
    let mut attempted = false;
    let mut failure = None;
    let mut output = None;
    revision
        .try_clone_into_slots_with_native_birth_v1(
            &mut backing,
            &mut output,
            &mut attempted,
            &mut failure,
            |bytes, birth| {
                assert_eq!(bytes, 8);
                Ok::<_, u8>(birth())
            },
        )
        .expect("same sealed revision clone");
    assert_eq!(output.as_ref(), Some(&revision));
    assert!(failure.is_none());
}

#[test]
fn unit_occupied_slots_preserve_typed_output_backing_and_first_failure() {
    let prior = Box::new(97_u8);
    let pointer = core::ptr::from_ref(prior.as_ref());
    let mut failure = Some(Failure::Admission(prior));
    let mut attempted = false;
    let mut backing = String::new();
    assert_eq!(
        copy(
            "unused",
            &mut backing,
            &mut attempted,
            &mut failure,
            |_, _| panic!("occupied full error must not poll")
        ),
        Err(Refusal::OccupiedOutput)
    );
    assert!(!attempted);
    assert_eq!(
        core::ptr::from_ref(
            failure
                .as_ref()
                .expect("prior")
                .admission_failure_v1()
                .expect("admission")
                .as_ref()
        ),
        pointer
    );

    for mut backing in [String::from("prior"), String::with_capacity(9)] {
        let pointer = backing.as_ptr();
        let capacity = backing.capacity();
        let prior = backing.clone();
        let mut failure = None::<Failure<u8>>;
        assert_eq!(
            copy(
                "unused",
                &mut backing,
                &mut attempted,
                &mut failure,
                |_, _| panic!("occupied backing must not poll")
            ),
            Err(Refusal::OccupiedOutput)
        );
        assert_eq!(backing, prior);
        assert_eq!(backing.as_ptr(), pointer);
        assert_eq!(backing.capacity(), capacity);
        assert!(!attempted);
    }
    let repo = RepoId::new("repo/test").expect("repo");
    let mut output = Some(RepoId::new("repo/prior").expect("prior"));
    let pointer = output.as_ref().expect("prior").as_str().as_ptr();
    let mut failure = None::<Failure<u8>>;
    assert_eq!(
        repo.try_clone_into_slots_with_native_birth_v1(
            &mut backing,
            &mut output,
            &mut attempted,
            &mut failure,
            |_, _| panic!("occupied typed output must not poll")
        ),
        Err(Refusal::OccupiedOutput)
    );
    assert_eq!(output.as_ref().expect("prior").as_str().as_ptr(), pointer);
    assert_eq!(backing.capacity(), 0);
    assert!(!attempted);
}

#[test]
fn unit_empty_copy_and_prebirth_refusal_cannot_be_reentered() {
    let mut backing = String::new();
    let mut attempted = false;
    let mut failure = None;
    copy(
        "",
        &mut backing,
        &mut attempted,
        &mut failure,
        |bytes, birth| {
            assert_eq!(bytes, 0);
            Ok::<_, u8>(birth())
        },
    )
    .expect("zero-byte copy");
    assert_eq!(backing.capacity(), 0);
    assert_eq!(
        copy(
            "unused",
            &mut backing,
            &mut attempted,
            &mut failure,
            |_, _| panic!("empty completed attempt must not poll")
        ),
        Err(Refusal::UsedData)
    );

    let mut attempted = false;
    assert_eq!(
        copy(
            "repo/test",
            &mut backing,
            &mut attempted,
            &mut failure,
            |_, _| Err(101_u8)
        ),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(backing.capacity(), 0);
    assert_eq!(failure.take(), Some(Failure::Admission(101)));
    assert_eq!(
        copy(
            "unused",
            &mut backing,
            &mut attempted,
            &mut failure,
            |_, _| panic!("refused attempt must not poll")
        ),
        Err(Refusal::UsedData)
    );
}

#[test]
fn repeated_successful_callback_keeps_partial_backing_and_protocol_cause() {
    let mut backing = String::new();
    let mut attempted = false;
    let mut failure = None;
    assert_eq!(
        copy(
            "repo/test",
            &mut backing,
            &mut attempted,
            &mut failure,
            |bytes, birth| {
                assert_eq!(bytes, 9);
                assert!(birth());
                assert!(!birth());
                Ok::<_, u8>(true)
            }
        ),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(failure, Some(Failure::InvalidNativeProducer));
    assert!(backing.is_empty());
    assert_eq!(backing.capacity(), 9);
}
