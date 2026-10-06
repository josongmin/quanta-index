use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use unicode_normalization::{
    try_for_each_nfc_with_native_admission_v1, try_is_nfc_with_native_admission_v1,
    NativeNormalizationAdmissionV1, NativeNormalizationErrorV1 as Error,
    NativeNormalizationScratchDemandV1 as Demand, NativeNormalizationScratchOwnerV1 as Owner,
    UnicodeNormalization,
};

thread_local! {
    static RECORD: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static LIVE_BYTES: Cell<usize> = const { Cell::new(0) };
    static FAIL_NEXT: Cell<bool> = const { Cell::new(false) };
}

// Instrumentation only: no original authority, quota, allocator replacement
// policy or thread-local production control is introduced.
struct AllocationProbe;
#[global_allocator]
static ALLOCATOR: AllocationProbe = AllocationProbe;
unsafe impl GlobalAlloc for AllocationProbe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL_NEXT
            .try_with(|flag| flag.replace(false))
            .unwrap_or(false)
        {
            return std::ptr::null_mut();
        }
        let value = unsafe { System.alloc(layout) };
        if !value.is_null() && RECORD.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
            LIVE_BYTES.with(|bytes| bytes.set(bytes.get() + layout.size()));
        }
        value
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if RECORD.try_with(Cell::get).unwrap_or(false) {
            LIVE_BYTES.with(|bytes| bytes.set(bytes.get().checked_sub(layout.size()).unwrap()));
        }
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if FAIL_NEXT
            .try_with(|flag| flag.replace(false))
            .unwrap_or(false)
        {
            return std::ptr::null_mut();
        }
        let value = unsafe { System.realloc(pointer, layout, new_size) };
        if !value.is_null() && RECORD.try_with(Cell::get).unwrap_or(false) {
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
