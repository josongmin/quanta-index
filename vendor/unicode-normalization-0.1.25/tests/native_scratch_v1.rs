use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use unicode_normalization::{
    try_for_each_nfc_into_with_native_admission_v1, try_for_each_nfc_with_native_admission_v1,
    try_is_nfc_into_with_native_admission_v1, try_is_nfc_with_native_admission_v1,
    NativeNormalizationAdmissionV1, NativeNormalizationDataRefusalV1 as Refusal,
    NativeNormalizationDataV1 as Data, NativeNormalizationErrorV1 as Error,
    NativeNormalizationOutcomeV1 as Outcome, NativeNormalizationScratchDemandV1 as Demand,
    NativeNormalizationScratchOwnerV1 as Owner, UnicodeNormalization,
};

thread_local! {
    static RECORD: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static LIVE_BYTES: Cell<usize> = const { Cell::new(0) };
    static FAIL_NEXT: Cell<bool> = const { Cell::new(false) };
}

// Allocation instrumentation must not manufacture a zero/false observation
// when its thread-local state is unavailable. The probe's const Cell keys
// have no destructors; unavailability is a fatal instrumentation invariant.
// Abort rather than unwind through GlobalAlloc.
fn probe_tls<T>(value: Result<T, std::thread::AccessError>) -> T {
    match value {
        Ok(value) => value,
        Err(_unavailable_probe) => std::process::abort(),
    }
}

// Instrumentation only: no original authority, quota, allocator replacement
// policy or thread-local production control is introduced.
struct AllocationProbe;
#[global_allocator]
static ALLOCATOR: AllocationProbe = AllocationProbe;
unsafe impl GlobalAlloc for AllocationProbe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if probe_tls(FAIL_NEXT.try_with(|flag| flag.replace(false))) {
            return std::ptr::null_mut();
        }
        let value = unsafe { System.alloc(layout) };
        if !value.is_null() && probe_tls(RECORD.try_with(Cell::get)) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
            LIVE_BYTES.with(|bytes| bytes.set(bytes.get() + layout.size()));
        }
        value
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if probe_tls(RECORD.try_with(Cell::get)) {
            LIVE_BYTES.with(|bytes| bytes.set(bytes.get().checked_sub(layout.size()).unwrap()));
        }
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if probe_tls(FAIL_NEXT.try_with(|flag| flag.replace(false))) {
            return std::ptr::null_mut();
        }
        let value = unsafe { System.realloc(pointer, layout, new_size) };
        if !value.is_null() && probe_tls(RECORD.try_with(Cell::get)) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
            LIVE_BYTES.with(|bytes| bytes.set(bytes.get() - layout.size() + new_size));
        }
        value
    }
}

struct Policy<'a> {
    cause: &'a Cell<u32>,
    remaining_work: u64,
    ceiling: usize,
    retained: [usize; 3],
    peak: usize,
    demands: Vec<Demand>,
    releases: Vec<Owner>,
    native_calls: usize,
    malformed: u8,
    fail_native: bool,
}
impl<'a> Policy<'a> {
    fn new(cause: &'a Cell<u32>) -> Self {
        Self {
            cause,
            remaining_work: u64::MAX,
            ceiling: usize::MAX,
            retained: [0; 3],
            peak: 0,
            demands: Vec::with_capacity(32),
            releases: Vec::with_capacity(3),
            native_calls: 0,
            malformed: 0,
            fail_native: false,
        }
    }
}
fn slot(owner: Owner) -> usize {
    match owner {
        Owner::Decomposition => 0,
        Owner::Recomposition => 1,
        Owner::Sort => 2,
    }
}
impl<'a> NativeNormalizationAdmissionV1 for Policy<'a> {
    type Error = &'a Cell<u32>;
    fn checkpoint_work_v1(&mut self, units: u64) -> Result<(), Self::Error> {
        let Some(next) = self.remaining_work.checked_sub(units) else {
            return Err(self.cause);
        };
        self.remaining_work = next;
        Ok(())
    }
    fn native_birth_v1(
        &mut self,
        demand: Demand,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error> {
        let index = slot(demand.owner_v1);
        assert_eq!(self.retained[index], demand.current_bytes_v1);
        self.demands.push(demand);
        let peak = self.retained.iter().sum::<usize>() + demand.new_bytes_v1;
        self.peak = self.peak.max(peak);
        if peak > self.ceiling {
            return Err(self.cause);
        }
        if self.malformed == 1 {
            return Ok(true);
        }
        FAIL_NEXT.with(|flag| flag.set(self.fail_native));
        let success = birth();
        self.native_calls += 1;
        if success {
            self.retained[index] = demand.new_bytes_v1;
        }
        if self.malformed == 2 {
            assert!(!birth());
        }
        Ok(if self.malformed == 3 {
            !success
        } else {
            success
        })
    }
    fn release_scratch_v1(&mut self, owner: Owner) {
        // Real deallocation precedes admission release, including error paths.
        if owner != Owner::Sort {
            LIVE_BYTES.with(|bytes| assert_eq!(bytes.get(), 0));
        }
        self.retained[slot(owner)] = 0;
        self.releases.push(owner);
    }
}

#[test]
fn long_nfc_stream_preserves_fixed_stable_order_and_releases_all_grants_v1() {
    let cause = Cell::new(59);
    let input = format!("a{}\u{300}", "\u{315}".repeat(513));
    let expected = format!("à{}", "\u{315}".repeat(513));
    let mut output = String::with_capacity(expected.len());
    let mut policy = Policy::new(&cause);
    let (result, actual_births) = measured(|| {
        try_for_each_nfc_with_native_admission_v1(&input, &mut policy, |scalar| {
            output.push(scalar);
            Ok(())
        })
    });
    assert!(result.is_ok());
    assert_eq!(output, expected);
    assert_eq!(actual_births, policy.native_calls);
    assert!(policy.releases.contains(&Owner::Sort));
    assert_eq!(policy.retained, [0; 3]);

    let mut refused = Policy::new(&cause);
    refused.ceiling = 1;
    let (result, actual_births) = measured(|| {
        try_for_each_nfc_with_native_admission_v1(&input, &mut refused, |_scalar| Ok(()))
    });
    assert!(matches!(result, Err(Error::Admission(value)) if std::ptr::eq(value, &cause)));
    assert_eq!(actual_births, refused.native_calls);
    assert_eq!(refused.retained, [0; 3]);
}
fn measured<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RECORD.with(|flag| flag.set(false));
        }
    }
    ALLOCATIONS.with(|count| count.set(0));
    LIVE_BYTES.with(|bytes| bytes.set(0));
    RECORD.with(|flag| flag.set(true));
    let reset = Reset;
    let result = operation();
    drop(reset);
    LIVE_BYTES.with(|bytes| assert_eq!(bytes.get(), 0));
    (result, ALLOCATIONS.with(Cell::get))
}

#[test]
fn controlled_nfc_uses_same_algorithm_and_only_admitted_scratch_births_v1() {
    let cause = Cell::new(37);
    let inputs = [
        "",
        "repo-main",
        "é",
        "e\u{301}",
        "가",
        "\u{1100}\u{1161}",
        "a\u{315}\u{300}",
        "a\u{300}\u{315}",
        "🦀",
        "ᾂ",
    ];
    for input in inputs {
        let expected = input.nfc().eq(input.chars());
        let mut policy = Policy::new(&cause);
        let (result, actual_births) =
            measured(|| try_is_nfc_with_native_admission_v1(input, &mut policy));
        assert_eq!(result.unwrap(), expected);
        assert_eq!(actual_births, policy.native_calls);
        assert_eq!(policy.retained, [0; 3]);
        assert_eq!(
            policy.releases,
            [Owner::Decomposition, Owner::Recomposition]
        );
    }
    // The longest admitted combining run reaches the canonical stable sort's
    // stack-only boundary. An initial starter also reaches recomposition spill.
    for input in ["\u{344}".repeat(256), format!("a{}", "\u{315}".repeat(255))] {
        let expected = input.nfc().eq(input.chars());
        let mut policy = Policy::new(&cause);
        let (result, actual_births) =
            measured(|| try_is_nfc_with_native_admission_v1(&input, &mut policy));
        assert_eq!(result.unwrap(), expected);
        assert_eq!(
            actual_births, policy.native_calls,
            "no hidden stable-sort allocation"
        );
    }
}

#[test]
fn original_borrowed_work_and_native_short_refuse_before_birth_v1() {
    let cause = Cell::new(41);
    let input = format!("a{}", "\u{315}".repeat(40));
    let mut sufficient = Policy::new(&cause);
    let (result, _) = measured(|| try_is_nfc_with_native_admission_v1(&input, &mut sufficient));
    assert!(result.unwrap());
    let exact_peak = sufficient.peak;
    let mut exact = Policy::new(&cause);
    exact.ceiling = exact_peak;
    let (result, _) = measured(|| try_is_nfc_with_native_admission_v1(&input, &mut exact));
    assert!(result.unwrap());
    let mut short = Policy::new(&cause);
    short.ceiling = exact_peak - 1;
    let (result, actual_births) =
        measured(|| try_is_nfc_with_native_admission_v1(&input, &mut short));
    assert!(matches!(result, Err(Error::Admission(value)) if std::ptr::eq(value, &cause)));
    assert_eq!(actual_births, short.native_calls);
    assert!(short.native_calls < exact.native_calls);
    let mut stopped = Policy::new(&cause);
    stopped.remaining_work = 0;
    let (result, actual_births) =
        measured(|| try_is_nfc_with_native_admission_v1(&input, &mut stopped));
    assert!(matches!(result, Err(Error::Admission(value)) if std::ptr::eq(value, &cause)));
    assert_eq!(actual_births, 0);
    assert_eq!(stopped.native_calls, 0);
}

#[test]
fn native_allocator_failure_and_invalid_birth_protocol_are_distinct_v1() {
    let cause = Cell::new(43);
    let input = "\u{315}".repeat(10);
    let mut failed = Policy::new(&cause);
    failed.fail_native = true;
    let (result, actual_births) =
        measured(|| try_is_nfc_with_native_admission_v1(&input, &mut failed));
    assert!(matches!(result, Err(Error::NativeAllocationFailed)));
    assert_eq!(actual_births, 0);
    assert_eq!(failed.native_calls, 1);
    for malformed in 1..=3 {
        let mut policy = Policy::new(&cause);
        policy.malformed = malformed;
        let (result, actual_births) =
            measured(|| try_is_nfc_with_native_admission_v1(&input, &mut policy));
        assert!(matches!(result, Err(Error::InvalidNativeProducer)));
        assert_eq!(actual_births, usize::from(malformed != 1));
    }
}

#[test]
fn external_identity_scratch_survives_return_and_rejects_reuse_without_poll_v1() {
    let cause = Cell::new(61);
    let input = format!("q{}", "\u{301}".repeat(20));
    let mut policy = Policy::new(&cause);
    let mut data = Data::new_v1();
    let (_, actual_births) = measured(|| {
        assert!(try_is_nfc_into_with_native_admission_v1(&input, &mut data, &mut policy).is_ok());
        assert!(matches!(data.result_v1(), Some(Ok(Outcome::IsNfc(true)))));
        assert!(!data.is_fresh_v1());
        LIVE_BYTES.with(|bytes| assert!(bytes.get() > 0));
        assert!(policy.retained[0] > 0 && policy.retained[1] > 0);
        assert!(policy.releases.is_empty());
        let calls = policy.native_calls;
        let work = policy.remaining_work;
        assert_eq!(
            try_is_nfc_into_with_native_admission_v1("other", &mut data, &mut policy),
            Err(Refusal::UsedData)
        );
        assert_eq!(policy.native_calls, calls);
        assert_eq!(policy.remaining_work, work);
        assert!(matches!(data.result_v1(), Some(Ok(Outcome::IsNfc(true)))));
        // Simulated caller terminal point: real buffers retire before grants.
        data.release_scratch_v1(&mut policy);
        data.release_scratch_v1(&mut policy);
        assert_eq!(
            policy.releases,
            [Owner::Decomposition, Owner::Recomposition]
        );
        assert_eq!(policy.retained, [0; 3]);
        assert!(matches!(data.result_v1(), Some(Ok(Outcome::IsNfc(true)))));
    });
    assert_eq!(actual_births, policy.native_calls);
    let mut occupied = Some(Ok(Outcome::Streamed));
    assert_eq!(
        data.result_into_slot_v1(&mut occupied),
        Err(Refusal::OccupiedOutput)
    );
    assert!(matches!(occupied, Some(Ok(Outcome::Streamed))));
    let mut result = None;
    data.result_into_slot_v1(&mut result).unwrap();
    assert!(matches!(result, Some(Ok(Outcome::IsNfc(true)))));
    assert_eq!(
        data.result_into_slot_v1(&mut None),
        Err(Refusal::MissingResult)
    );
}

// Fixed storage keeps instrumentation independent of policy allocations.
// The non-Clone cause models a full original error, including a late refusal.
struct LateRefusal {
    cause: Option<Box<u32>>,
    owner: Owner,
    retained: [usize; 3],
    calls: usize,
    polls: usize,
    releases: usize,
}
impl NativeNormalizationAdmissionV1 for LateRefusal {
    type Error = Box<u32>;
    fn checkpoint_work_v1(&mut self, _units: u64) -> Result<(), Self::Error> {
        self.polls += 1;
        Ok(())
    }
    fn native_birth_v1(
        &mut self,
        demand: Demand,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error> {
        let index = slot(demand.owner_v1);
        assert_eq!(self.retained[index], demand.current_bytes_v1);
        self.calls += 1;
        let success = birth();
        assert!(success);
        self.retained[index] = demand.new_bytes_v1;
        if demand.owner_v1 == self.owner {
            return Err(self.cause.take().expect("exactly one refusal"));
        }
        Ok(success)
    }
    fn release_scratch_v1(&mut self, owner: Owner) {
        LIVE_BYTES.with(|bytes| assert_eq!(bytes.get(), 0));
        self.retained[slot(owner)] = 0;
        self.releases += 1;
    }
}

#[test]
fn external_data_retains_actual_late_birth_and_full_noncopy_error_v1() {
    for owner in [Owner::Decomposition, Owner::Recomposition, Owner::Sort] {
        let input = format!("a{}\u{300}", "\u{315}".repeat(513));
        let cause = Box::new(67);
        let pointer = std::ptr::from_ref(cause.as_ref());
        let mut policy = LateRefusal {
            cause: Some(cause),
            owner,
            retained: [0; 3],
            calls: 0,
            polls: 0,
            releases: 0,
        };
        let mut data = Data::new_v1();
        let (_, actual_births) = measured(|| {
            assert!(try_for_each_nfc_into_with_native_admission_v1(
                &input,
                &mut data,
                &mut policy,
                |_| Ok(())
            )
            .is_err());
            assert!(
                matches!(data.result_v1(), Some(Err(Error::Admission(cause))) if std::ptr::from_ref(cause.as_ref()) == pointer && **cause == 67)
            );
            LIVE_BYTES.with(|bytes| assert!(bytes.get() > 0));
            assert!(policy.retained[slot(owner)] > 0);
            assert_eq!(policy.releases, 0);
            let polls = policy.polls;
            let calls = policy.calls;
            assert!(try_for_each_nfc_into_with_native_admission_v1(
                "other",
                &mut data,
                &mut policy,
                |_| panic!("used DATA must not emit")
            )
            .is_err());
            assert_eq!(policy.polls, polls);
            assert_eq!(policy.calls, calls);
            data.release_scratch_v1(&mut policy);
            assert_eq!(policy.retained, [0; 3]);
            assert_eq!(
                policy.releases,
                if owner == Owner::Decomposition { 2 } else { 3 }
            );
            assert!(
                matches!(data.result_v1(), Some(Err(Error::Admission(cause))) if std::ptr::from_ref(cause.as_ref()) == pointer)
            );
        });
        assert_eq!(actual_births, policy.calls);
        // Cause was allocated before the allocator probe. Transfer/drop after
        // it stops, proving its allocation survived physical scratch release.
        let mut result = None;
        data.result_into_slot_v1(&mut result).unwrap();
        assert!(
            matches!(result, Some(Err(Error::Admission(ref cause))) if std::ptr::from_ref(cause.as_ref()) == pointer)
        );
    }
}

#[test]
fn external_stream_retains_emitter_error_and_scratch_until_terminal_v1() {
    let input = format!("q{}", "\u{301}".repeat(20));
    let cause = Box::new(73);
    let pointer = std::ptr::from_ref(cause.as_ref());
    let mut cause = Some(cause);
    let mut policy = LateRefusal {
        cause: None,
        owner: Owner::Sort,
        retained: [0; 3],
        calls: 0,
        polls: 0,
        releases: 0,
    };
    let mut data = Data::new_v1();
    let (_, actual_births) = measured(|| {
        assert!(try_for_each_nfc_into_with_native_admission_v1(
            &input,
            &mut data,
            &mut policy,
            |_| Err(cause
                .take()
                .expect("first emitter refusal stops normalization")),
        )
        .is_err());
        assert!(
            matches!(data.result_v1(), Some(Err(Error::Admission(cause)))
            if std::ptr::from_ref(cause.as_ref()) == pointer && **cause == 73)
        );
        LIVE_BYTES.with(|bytes| assert!(bytes.get() > 0));
        assert_eq!(policy.releases, 0);
        data.release_scratch_v1(&mut policy);
        assert_eq!(policy.retained, [0; 3]);
    });
    assert_eq!(actual_births, policy.calls);
    // DATA has no input lifetime; even the full error survives source drop.
    drop(input);
    assert!(
        matches!(data.result_v1(), Some(Err(Error::Admission(cause)))
        if std::ptr::from_ref(cause.as_ref()) == pointer)
    );
}

#[test]
fn external_stream_reuses_and_grows_sort_backing_with_fixed_stable_output_v1() {
    let cause = Cell::new(71);
    let input = format!(
        "a{}\u{300}b{}\u{300}b{}\u{300}",
        "\u{315}".repeat(513),
        "\u{315}".repeat(520),
        "\u{315}".repeat(513)
    );
    let expected = format!(
        "à{}b\u{300}{}b\u{300}{}",
        "\u{315}".repeat(513),
        "\u{315}".repeat(520),
        "\u{315}".repeat(513)
    );
    let mut output = String::with_capacity(expected.len());
    let mut policy = Policy::new(&cause);
    let mut data = Data::new_v1();
    let (_, actual_births) = measured(|| {
        assert!(try_for_each_nfc_into_with_native_admission_v1(
            &input,
            &mut data,
            &mut policy,
            |scalar| {
                output.push(scalar);
                Ok(())
            }
        )
        .is_ok());
        assert_eq!(output, expected);
        assert!(matches!(data.result_v1(), Some(Ok(Outcome::Streamed))));
        assert!(policy.retained.iter().all(|bytes| *bytes > 0));
        assert!(policy.releases.is_empty());
        let mut sorts = policy
            .demands
            .iter()
            .filter(|demand| demand.owner_v1 == Owner::Sort);
        assert_eq!(sorts.next().unwrap().current_bytes_v1, 0);
        assert!(sorts.next().unwrap().current_bytes_v1 > 0);
        assert!(sorts.next().is_none());
        data.release_scratch_v1(&mut policy);
        assert_eq!(policy.retained, [0; 3]);
        assert_eq!(
            policy.releases,
            [Owner::Sort, Owner::Decomposition, Owner::Recomposition]
        );
    });
    assert_eq!(actual_births, policy.native_calls);
}
