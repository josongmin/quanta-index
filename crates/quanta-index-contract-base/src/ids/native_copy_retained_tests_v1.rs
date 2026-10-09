//! Authored owner regressions; mock admission is not Original Source proof.

use super::{
    NativeIdentityCopyDataV1 as Data, NativeIdentityCopyErrorV1 as Failure,
    NativeIdentityCopyPhaseV1 as Phase, NativeIdentityCopyRefusalV1 as Refusal, RepoId, RevisionId,
    admit_native_copy_backing_v1, begin_native_copy_attempt_v1, reserve_native_copy_step_v1,
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
fn physical_reserve_is_external_before_admission_resumes_and_polls() {
    let expected = String::new()
        .try_reserve_exact(usize::MAX)
        .expect_err("capacity overflow");
    let mut backing = String::new();
    let mut data = Data::new_v1();
    begin_native_copy_attempt_v1(&backing, &mut data).expect("fresh external phase");
    assert!(!reserve_native_copy_step_v1(
        usize::MAX,
        &mut backing,
        &mut data
    ));
    // Observe the SAME external slot before admission has finished or the
    // composite failure has been materialized. No forged str or allocator.
    assert!(data.phase == Phase::Admitting);
    assert_eq!(data.reserve_failure.as_ref(), Some(&expected));
    assert!(data.failure_v1().is_none());
    let address = core::ptr::from_ref(data.reserve_failure.as_ref().expect("physical cause"));
    assert!(!reserve_native_copy_step_v1(0, &mut backing, &mut data));
    assert_eq!(
        core::ptr::from_ref(data.reserve_failure.as_ref().expect("first cause")),
        address
    );
    let original = Box::new(79_u8);
    let original_address = core::ptr::from_ref(original.as_ref());
    assert_eq!(
        data.refuse_admission_v1(original),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(data.reserve_failure_v1(), Some(&expected));
    assert_eq!(
        core::ptr::from_ref(data.admission_failure_v1().expect("original").as_ref()),
        original_address
    );
}

#[test]
fn real_reserve_cause_survives_later_protocol_and_admission_failures() {
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
        let mut data = Data::new_v1();
        begin_native_copy_attempt_v1(&backing, &mut data).expect("begin");
        let calls = Cell::new(0_u32);
        assert_eq!(
            admit_native_copy_backing_v1(usize::MAX, &mut backing, &mut data, |bytes, birth| {
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
                    Disposition::RefuseLate => Err(original.take().expect("original")),
                }
            }),
            Err(Refusal::OperationRefused)
        );
        assert_eq!(calls.get(), 1);
        assert!(!data.is_fresh_v1());
        assert_eq!(backing.capacity(), 0);
        assert_eq!(data.reserve_failure_v1(), Some(&expected));
        match disposition {
            Disposition::ReturnFailure => assert!(matches!(
                data.failure_v1(),
                Some(Failure::NativeAllocationFailed(_))
            )),
            Disposition::ReportSuccess | Disposition::Repeat => assert!(matches!(
                data.failure_v1(),
                Some(Failure::InvalidNativeProducerAfterReserveFailure(_))
            )),
            Disposition::RefuseLate => {
                assert!(matches!(
                    data.failure_v1(),
                    Some(Failure::AdmissionAfterReserveFailure { .. })
                ));
                assert_eq!(
                    core::ptr::from_ref(data.admission_failure_v1().expect("original").as_ref()),
                    original_pointer
                );
            }
        }
        let address = core::ptr::from_ref(data.failure_v1().expect("retained"));
        assert_eq!(
            copy("unused", &mut backing, &mut data, |_, _| panic!(
                "used DATA must not poll"
            )),
            Err(Refusal::UsedData)
        );
        assert_eq!(
            core::ptr::from_ref(data.failure_v1().expect("first cause")),
            address
        );
        let mut occupied = Some(Failure::Admission(Box::new(89_u8)));
        assert_eq!(
            data.failure_into_slot_v1(&mut occupied),
            Err(Refusal::OccupiedOutput)
        );
        assert_eq!(
            core::ptr::from_ref(data.failure_v1().expect("first cause")),
            address
        );
        let mut output = None;
        data.failure_into_slot_v1(&mut output)
            .expect("external pure transfer");
        assert_eq!(
            output
                .as_ref()
                .expect("complete error")
                .reserve_failure_v1(),
            Some(&expected)
        );
        assert_eq!(
            data.failure_into_slot_v1(&mut None),
            Err(Refusal::MissingResult)
        );
        assert_eq!(
            copy("unused", &mut backing, &mut data, |_, _| panic!(
                "transferred DATA must not poll"
            )),
            Err(Refusal::UsedData)
        );
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
    let original = Box::new(97_u8);
    let pointer = core::ptr::from_ref(original.as_ref());
    let mut backing = String::new();
    let mut data = Data::new_v1();
    assert_eq!(
        copy("repo/test", &mut backing, &mut data, |bytes, birth| {
            assert_eq!(bytes, 9);
            funding = Some(Funding(Rc::clone(&drops)));
            assert!(birth());
            Err::<bool, _>(original)
        }),
        Err(Refusal::OperationRefused)
    );
    assert!(backing.is_empty());
    assert_eq!(backing.capacity(), 9);
    assert_eq!(
        core::ptr::from_ref(data.admission_failure_v1().expect("original").as_ref()),
        pointer
    );
    assert!(data.reserve_failure_v1().is_none());
    assert_eq!(drops.get(), 0);
    let backing_pointer = backing.as_ptr();
    assert_eq!(
        copy("unused", &mut backing, &mut data, |_, _| panic!(
            "born backing must stay"
        )),
        Err(Refusal::OccupiedOutput)
    );
    assert_eq!(backing.as_ptr(), backing_pointer);
    assert_eq!(backing.capacity(), 9);
    drop(backing);
    drop(data);
    assert_eq!(drops.get(), 0);
    drop(funding);
    assert_eq!(drops.get(), 1);
}

#[test]
fn unit_typed_clones_keep_private_bytes_and_one_shot_state() {
    let repo = RepoId::new("répo/../%").expect("canonical repo");
    let mut backing = String::new();
    let mut output = None;
    let mut data = Data::new_v1();
    repo.try_clone_into_slots_with_native_birth_v1(
        &mut backing,
        &mut output,
        &mut data,
        |bytes, birth| {
            assert_eq!(bytes, 10);
            Ok::<_, u8>(birth())
        },
    )
    .expect("same sealed clone");
    assert_eq!(output.as_ref(), Some(&repo));
    assert!(data.is_complete_v1());
    assert!(data.failure_v1().is_none());
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
            &mut data,
            |_, _| panic!("used clone must not poll")
        ),
        Err(Refusal::UsedData)
    );
    assert_eq!(copied, repo);
    let revision = RevisionId::new("rev/test").expect("canonical revision");
    let mut data = Data::new_v1();
    let mut output = None;
    revision
        .try_clone_into_slots_with_native_birth_v1(
            &mut backing,
            &mut output,
            &mut data,
            |bytes, birth| {
                assert_eq!(bytes, 8);
                Ok::<_, u8>(birth())
            },
        )
        .expect("same sealed revision clone");
    assert_eq!(output.as_ref(), Some(&revision));
    assert!(data.failure_v1().is_none());
}

#[test]
fn unit_occupied_slots_preserve_typed_output_backing_and_fresh_data() {
    for mut backing in [String::from("prior"), String::with_capacity(9)] {
        let pointer = backing.as_ptr();
        let capacity = backing.capacity();
        let prior = backing.clone();
        let mut data = Data::<u8>::new_v1();
        assert_eq!(
            copy("unused", &mut backing, &mut data, |_, _| panic!(
                "occupied backing must not poll"
            )),
            Err(Refusal::OccupiedOutput)
        );
        assert_eq!(backing, prior);
        assert_eq!(backing.as_ptr(), pointer);
        assert_eq!(backing.capacity(), capacity);
        assert!(data.is_fresh_v1());
    }
    let repo = RepoId::new("repo/test").expect("repo");
    let mut output = Some(RepoId::new("repo/prior").expect("prior"));
    let pointer = output.as_ref().expect("prior").as_str().as_ptr();
    let mut backing = String::new();
    let mut data = Data::<u8>::new_v1();
    assert_eq!(
        repo.try_clone_into_slots_with_native_birth_v1(
            &mut backing,
            &mut output,
            &mut data,
            |_, _| panic!("occupied typed output must not poll")
        ),
        Err(Refusal::OccupiedOutput)
    );
    assert_eq!(output.as_ref().expect("prior").as_str().as_ptr(), pointer);
    assert_eq!(backing.capacity(), 0);
    assert!(data.is_fresh_v1());
}

#[test]
fn unit_empty_copy_and_prebirth_refusal_cannot_be_reentered() {
    let mut backing = String::new();
    let mut data = Data::new_v1();
    copy("", &mut backing, &mut data, |bytes, birth| {
        assert_eq!(bytes, 0);
        Ok::<_, u8>(birth())
    })
    .expect("zero-byte copy");
    assert!(data.is_complete_v1());
    assert_eq!(backing.capacity(), 0);
    assert_eq!(
        copy("unused", &mut backing, &mut data, |_, _| panic!(
            "completed DATA must not poll"
        )),
        Err(Refusal::UsedData)
    );
    let mut data = Data::new_v1();
    assert_eq!(
        copy("repo/test", &mut backing, &mut data, |_, _| Err(101_u8)),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(data.admission_failure_v1(), Some(&101));
    assert_eq!(backing.capacity(), 0);
    assert_eq!(
        copy("unused", &mut backing, &mut data, |_, _| panic!(
            "refused DATA must not poll"
        )),
        Err(Refusal::UsedData)
    );
}

#[test]
fn repeated_successful_callback_keeps_partial_backing_and_protocol_cause() {
    let mut backing = String::new();
    let mut data = Data::new_v1();
    assert_eq!(
        copy("repo/test", &mut backing, &mut data, |bytes, birth| {
            assert_eq!(bytes, 9);
            assert!(birth());
            assert!(!birth());
            Ok::<_, u8>(true)
        }),
        Err(Refusal::OperationRefused)
    );
    assert!(matches!(
        data.failure_v1(),
        Some(Failure::InvalidNativeProducer)
    ));
    assert!(backing.is_empty());
    assert_eq!(backing.capacity(), 9);
}
