//! Contract and fixed predecessor-trace tests, independent of the retired fork.

use super::*;
use unicode_normalization::UnicodeNormalization as _;

#[path = "compat_goldens.rs"]
mod compat_goldens;

const HASH_INITIAL: u64 = 14_695_981_039_346_656_037;
const HASH_PRIME: u64 = 1_099_511_628_211;

struct Trace {
    live: [usize; 3],
    work: u64,
    max_work: Option<u64>,
    max_peak: Option<usize>,
    hash: u64,
    events: u64,
    released: Vec<ScratchOwner>,
}

impl Trace {
    fn new(max_work: Option<u64>, max_peak: Option<usize>) -> Self {
        Self {
            live: [0; 3],
            work: 0,
            max_work,
            max_peak,
            hash: HASH_INITIAL,
            events: 0,
            released: Vec::new(),
        }
    }

    fn add(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.hash ^= u64::from(byte);
            self.hash = self.hash.wrapping_mul(HASH_PRIME);
        }
        self.events = self.events.saturating_add(1);
    }
}

fn slot(owner: ScratchOwner) -> usize {
    match owner {
        ScratchOwner::Decomposition => 0,
        ScratchOwner::Recomposition => 1,
        ScratchOwner::Sort => 2,
    }
}

impl ScratchAdmission for Trace {
    type Error = ();

    fn checkpoint(&mut self, units: u64) -> Result<(), ()> {
        self.add(1);
        self.add(units);
        let next = self.work.checked_add(units).ok_or(())?;
        if self.max_work.is_some_and(|max| next > max) {
            return Err(());
        }
        self.work = next;
        Ok(())
    }

    fn checkpoint_prepaid(&mut self) -> Result<(), ()> {
        // Predecessor goldens cover the legacy debit and backing trace.
        // The paid-sort cancellation test independently checks new bulk polls.
        Ok(())
    }

    fn birth(
        &mut self,
        demand: ScratchDemand,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, ()> {
        let index = slot(demand.owner);
        assert_eq!(self.live.get(index), Some(&demand.current_bytes));
        self.add(2);
        self.add(u64::try_from(index).unwrap());
        self.add(u64::try_from(demand.current_bytes).unwrap());
        self.add(u64::try_from(demand.new_bytes).unwrap());
        let peak = self
            .live
            .iter()
            .try_fold(demand.new_bytes, |sum, value| sum.checked_add(*value))
            .ok_or(())?;
        if self.max_peak.is_some_and(|max| peak > max) {
            self.add(0);
            return Err(());
        }
        let born = birth();
        self.add(if born { 1 } else { 2 });
        if born {
            *self.live.get_mut(index).ok_or(())? = demand.new_bytes;
        }
        Ok(born)
    }

    fn release(&mut self, owner: ScratchOwner) {
        let index = slot(owner);
        self.add(3);
        self.add(u64::try_from(index).unwrap());
        if let Some(value) = self.live.get_mut(index) {
            *value = 0;
        }
        self.released.push(owner);
    }
}

fn output_hash(text: &str) -> u64 {
    text.bytes().fold(HASH_INITIAL, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(HASH_PRIME)
    })
}

#[test]
fn stream_matches_fixed_predecessor_result_and_event_traces() {
    let long = format!("A{}B", "\u{315}\u{300}".repeat(300));
    let repeated = "é".repeat(100);
    let inputs = [
        "",
        "ASCII",
        "e\u{301}",
        "\u{212a}",
        "\u{1100}\u{1161}\u{11a8}",
        "A\u{315}\u{300}",
        repeated.as_str(),
        long.as_str(),
    ];
    let settings = [
        (None, None, None),
        (Some(0), None, None),
        (Some(20), None, None),
        (Some(500), None, None),
        (None, Some(0), None),
        (None, Some(64), None),
        (None, Some(1024), None),
        (None, None, Some(1)),
    ];
    for (
        input_id,
        setting_id,
        expected_ok,
        expected_chars,
        expected_hash,
        expected_work,
        expected_events,
        expected_trace,
    ) in compat_goldens::STREAM
    {
        let input = inputs.get(input_id).unwrap();
        let (work, peak, fail_at) = *settings.get(setting_id).unwrap();
        let mut admission = Trace::new(work, peak);
        let mut output = String::new();
        let mut emitted = 0_usize;
        let result = for_each_nfc(input, &mut admission, |scalar| {
            if Some(emitted) == fail_at {
                return Err(());
            }
            emitted += 1;
            output.push(scalar);
            Ok(())
        });
        assert_eq!(
            u8::from(result.is_ok()),
            expected_ok,
            "input {input_id}, setting {setting_id}"
        );
        if result.is_err() {
            assert_eq!(result, Err(NfcError::Admission(())));
        }
        assert_eq!(
            (
                emitted,
                output_hash(&output),
                admission.work,
                admission.events,
                admission.hash
            ),
            (
                expected_chars,
                expected_hash,
                expected_work,
                expected_events,
                expected_trace
            ),
            "input {input_id}, setting {setting_id}"
        );
        assert!(input.nfc().collect::<String>().starts_with(&output));
        assert_eq!(admission.live, [0; 3]);
    }
}

#[test]
fn identity_data_matches_fixed_predecessor_traces_and_limit() {
    let limit = "x".repeat(512);
    let too_long = "x".repeat(513);
    let inputs = [
        "",
        "ASCII",
        "é",
        "e\u{301}",
        "A\u{315}\u{300}",
        limit.as_str(),
        too_long.as_str(),
    ];
    let verdicts = [
        Some(true),
        Some(true),
        Some(true),
        Some(false),
        Some(false),
        Some(true),
        None,
    ];
    for (index, ((expected_ok, work, events, trace), verdict)) in compat_goldens::IDENTITY
        .into_iter()
        .zip(verdicts)
        .enumerate()
    {
        let mut data = NormalizationDataV1::new_v1();
        let mut admission = Trace::new(None, None);
        let status = is_nfc_into_v1(inputs.get(index).unwrap(), &mut data, &mut admission);
        assert_eq!(u8::from(status.is_ok()), expected_ok);
        match verdict {
            Some(value) => assert_eq!(
                data.result_v1(),
                Some(Ok(NormalizationOutcomeV1::IsNfc(value)))
            ),
            None => assert_eq!(data.result_v1(), Some(Err(&NfcError::InputTooLong))),
        }
        data.release_scratch_v1(&mut admission);
        assert_eq!(
            (admission.work, admission.events, admission.hash),
            (work, events, trace),
            "identity {index}"
        );
        assert_eq!(admission.live, [0; 3]);
    }
}

#[test]
fn caller_owned_data_preserves_result_and_backing_until_release() {
    let input = format!("A{}B", "\u{315}\u{300}".repeat(300));
    let mut data = NormalizationDataV1::new_v1();
    let mut admission = Trace::new(None, None);
    let mut output = String::new();
    assert!(data.is_fresh_v1());
    assert_eq!(
        for_each_nfc_into_v1(&input, &mut data, &mut admission, |scalar| {
            output.push(scalar);
            Ok(())
        }),
        Ok(())
    );
    assert_eq!(output, input.nfc().collect::<String>());
    assert_eq!(data.result_v1(), Some(Ok(NormalizationOutcomeV1::Streamed)));
    assert!(admission.live.iter().any(|value| *value > 0));
    assert!(admission.released.is_empty());
    let events = admission.events;
    assert_eq!(
        for_each_nfc_into_v1("again", &mut data, &mut admission, |_| Ok(())),
        Err(NormalizationDataRefusalV1::UsedData)
    );
    assert_eq!(events, admission.events);
    let mut occupied = Some(Ok(NormalizationOutcomeV1::IsNfc(false)));
    assert_eq!(
        data.result_into_slot_v1(&mut occupied),
        Err(NormalizationDataRefusalV1::OccupiedOutput)
    );
    assert_eq!(occupied, Some(Ok(NormalizationOutcomeV1::IsNfc(false))));
    let mut result = None;
    assert_eq!(data.result_into_slot_v1(&mut result), Ok(()));
    assert_eq!(result, Some(Ok(NormalizationOutcomeV1::Streamed)));
    assert_eq!(
        data.result_into_slot_v1(&mut None),
        Err(NormalizationDataRefusalV1::MissingResult)
    );
    data.release_scratch_v1(&mut admission);
    data.release_scratch_v1(&mut admission);
    assert_eq!(
        admission.released,
        [
            ScratchOwner::Sort,
            ScratchOwner::Decomposition,
            ScratchOwner::Recomposition
        ]
    );
    assert_eq!(admission.live, [0; 3]);
}

#[test]
fn retained_refusal_preserves_cause_and_funding_until_release() {
    let input = format!("A{}B", "\u{315}\u{300}".repeat(300));
    for (work, peak) in [
        (Some(0), None),
        (Some(50), None),
        (Some(500), None),
        (None, Some(0)),
        (None, Some(64)),
        (None, Some(1_024)),
    ] {
        let mut data = NormalizationDataV1::new_v1();
        let mut admission = Trace::new(work, peak);
        let status = for_each_nfc_into_v1(&input, &mut data, &mut admission, |_| Ok(()));
        assert_eq!(status, Err(NormalizationDataRefusalV1::OperationRefused));
        assert_eq!(data.result_v1(), Some(Err(&NfcError::Admission(()))));
        assert!(admission.released.is_empty());
        data.release_scratch_v1(&mut admission);
        assert_eq!(admission.live, [0; 3]);
        let mut result = None;
        assert_eq!(data.result_into_slot_v1(&mut result), Ok(()));
        assert_eq!(result, Some(Err(NfcError::Admission(()))));
    }
}

#[test]
fn panic_keeps_caller_data_and_funding_without_release_callback() {
    let input = format!("A{}B", "\u{315}\u{300}".repeat(300));
    let mut data = NormalizationDataV1::new_v1();
    let mut admission = Trace::new(None, None);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _status = for_each_nfc_into_v1(&input, &mut data, &mut admission, |_| {
            panic!("emitter panic")
        });
    }));
    assert!(result.is_err());
    assert!(data.result_v1().is_none());
    assert!(!data.is_fresh_v1());
    assert_eq!(
        for_each_nfc_into_v1("again", &mut data, &mut admission, |_| Ok(())),
        Err(NormalizationDataRefusalV1::UsedData)
    );
    assert!(admission.live.iter().any(|value| *value > 0));
    assert!(admission.released.is_empty());
    drop(data);
}

#[test]
fn mixed_scalars_and_long_ascii_match_independent_iterator() {
    const SCALARS: [char; 14] = [
        'A', 'é', '\u{212a}', '\u{315}', '\u{300}', '\u{301}', '\u{327}', '\u{1100}', '\u{1161}',
        '\u{11a8}', '가', '각', '\u{1f82}', '\u{0340}',
    ];
    let mut state = 0x9e37_79b9_u32;
    for _ in 0..128 {
        let mut input = String::new();
        for _ in 0..23 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            input.push(
                *SCALARS
                    .get(usize::try_from(state).unwrap() % SCALARS.len())
                    .unwrap(),
            );
        }
        let mut admission = Trace::new(None, None);
        let mut output = String::new();
        for_each_nfc(&input, &mut admission, |scalar| {
            output.push(scalar);
            Ok(())
        })
        .unwrap();
        assert_eq!(output, input.nfc().collect::<String>());
        assert_eq!(admission.live, [0; 3]);
    }
    let input = "a".repeat(4096);
    let mut admission = Trace::new(None, Some(0));
    let mut output = String::new();
    for_each_nfc(&input, &mut admission, |scalar| {
        output.push(scalar);
        Ok(())
    })
    .unwrap();
    assert_eq!(output, input);
    assert_eq!(admission.live, [0; 3]);
}

#[test]
fn malformed_birth_protocol_is_rejected_without_publishing_a_result() {
    #[derive(Clone, Copy)]
    enum Fault {
        Skip,
        Twice,
        Flip,
    }
    struct InvalidAdmission {
        fault: Fault,
        calls: usize,
        releases: Vec<ScratchOwner>,
    }
    impl ScratchAdmission for InvalidAdmission {
        type Error = ();
        fn checkpoint(&mut self, _: u64) -> Result<(), ()> {
            Ok(())
        }
        fn birth(&mut self, _: ScratchDemand, birth: &mut dyn FnMut() -> bool) -> Result<bool, ()> {
            self.calls += 1;
            match self.fault {
                Fault::Skip => Ok(true),
                Fault::Twice => {
                    let first = birth();
                    let _second = birth();
                    Ok(first)
                }
                Fault::Flip => Ok(!birth()),
            }
        }
        fn release(&mut self, owner: ScratchOwner) {
            self.releases.push(owner);
        }
    }
    let input = format!("A{}", "\u{315}\u{300}".repeat(100));
    for fault in [Fault::Skip, Fault::Twice, Fault::Flip] {
        let mut admission = InvalidAdmission {
            fault,
            calls: 0,
            releases: Vec::new(),
        };
        let mut data = NormalizationDataV1::new_v1();
        let mut output = String::new();
        assert_eq!(
            for_each_nfc_into_v1(&input, &mut data, &mut admission, |scalar| {
                output.push(scalar);
                Ok(())
            }),
            Err(NormalizationDataRefusalV1::OperationRefused)
        );
        assert_eq!(
            data.result_v1(),
            Some(Err(&NfcError::InvalidNativeProducer))
        );
        assert!(output.is_empty());
        assert_eq!(admission.calls, 1);
        assert!(admission.releases.is_empty());
        data.release_scratch_v1(&mut admission);
        assert_eq!(
            admission.releases,
            [ScratchOwner::Decomposition, ScratchOwner::Recomposition]
        );
    }
}

#[test]
fn long_stream_cancels_during_paid_sort_and_keeps_backing_v1() {
    struct StopInsideSort {
        trace: Trace,
        checkpoints_after_sort_birth: usize,
        stop_at: Option<usize>,
    }

    impl ScratchAdmission for StopInsideSort {
        type Error = &'static str;

        fn checkpoint(&mut self, units: u64) -> Result<(), Self::Error> {
            if units == 0 && self.trace.live[slot(ScratchOwner::Sort)] != 0 {
                self.checkpoints_after_sort_birth += 1;
                if self.stop_at == Some(self.checkpoints_after_sort_birth) {
                    return Err("sort interrupted");
                }
            }
            self.trace.checkpoint(units).map_err(|()| "work")
        }

        fn birth(
            &mut self,
            demand: ScratchDemand,
            birth: &mut dyn FnMut() -> bool,
        ) -> Result<bool, Self::Error> {
            self.trace.birth(demand, birth).map_err(|()| "birth")
        }

        fn release(&mut self, owner: ScratchOwner) {
            self.trace.release(owner);
        }
    }

    let input = format!("A{}B", "\u{315}\u{300}".repeat(800));
    let admission = |stop_at| StopInsideSort {
        trace: Trace::new(None, None),
        checkpoints_after_sort_birth: 0,
        stop_at,
    };
    let mut output = String::new();
    // 1,600 marks take four chunks for each sort pass. Stop inside
    // initialization, the census, the stable scatter and the copy back.
    for stop_at in [3, 7, 11, 15] {
        let mut cancelled = admission(Some(stop_at));
        let mut data = NormalizationDataV1::new_v1();
        assert_eq!(
            for_each_nfc_into_v1(&input, &mut data, &mut cancelled, |scalar| {
                output.push(scalar);
                Ok(())
            }),
            Err(NormalizationDataRefusalV1::OperationRefused)
        );
        assert_eq!(
            data.result_v1(),
            Some(Err(&NfcError::Admission("sort interrupted")))
        );
        assert!(output.is_empty());
        assert_eq!(cancelled.checkpoints_after_sort_birth, stop_at);
        assert!(cancelled.trace.live[slot(ScratchOwner::Sort)] > 0);
        assert!(cancelled.trace.released.is_empty());
        data.release_scratch_v1(&mut cancelled);
        assert_eq!(cancelled.trace.live, [0; 3]);
    }

    let mut retry = admission(None);
    let mut retry_data = NormalizationDataV1::new_v1();
    for_each_nfc_into_v1(&input, &mut retry_data, &mut retry, |scalar| {
        output.push(scalar);
        Ok(())
    })
    .unwrap();
    assert_eq!(output, input.nfc().collect::<String>());
    retry_data.release_scratch_v1(&mut retry);
    assert_eq!(retry.trace.live, [0; 3]);
}

#[test]
fn uncapped_borrowed_compare_preserves_verdict_and_terminal_refusal_v1() {
    struct CompareAdmission {
        trace: Trace,
        checkpoints: usize,
        stop_at: Option<usize>,
    }

    impl ScratchAdmission for CompareAdmission {
        type Error = &'static str;

        fn checkpoint(&mut self, units: u64) -> Result<(), Self::Error> {
            self.checkpoints += 1;
            if self.stop_at == Some(self.checkpoints) {
                assert_eq!(units, 0);
                return Err("before canonical verdict");
            }
            self.trace.checkpoint(units).map_err(|()| "work")
        }

        fn birth(
            &mut self,
            demand: ScratchDemand,
            birth: &mut dyn FnMut() -> bool,
        ) -> Result<bool, Self::Error> {
            self.trace.birth(demand, birth).map_err(|()| "birth")
        }

        fn release(&mut self, owner: ScratchOwner) {
            self.trace.release(owner);
        }
    }

    let input = format!("A{}B", "\u{315}\u{300}".repeat(800));
    let canonical = input.nfc().collect::<String>();
    for (source, expected) in [(&input, false), (&canonical, true)] {
        let mut capped = NormalizationDataV1::new_v1();
        let mut capped_admission = Trace::new(None, None);
        assert_eq!(
            is_nfc_into_v1(source, &mut capped, &mut capped_admission),
            Err(NormalizationDataRefusalV1::OperationRefused)
        );
        assert_eq!(capped.result_v1(), Some(Err(&NfcError::InputTooLong)));

        let scalars = source.nfd().count();
        let admission = |stop_at| CompareAdmission {
            trace: Trace::new(None, Some(40 * scalars)),
            checkpoints: 0,
            stop_at,
        };
        let mut measured = admission(None);
        let mut data = NormalizationDataV1::new_v1();
        assert_eq!(
            is_nfc_unbounded_into_v1(source, &mut data, &mut measured),
            Ok(())
        );
        assert_eq!(
            data.result_v1(),
            Some(Ok(NormalizationOutcomeV1::IsNfc(expected)))
        );
        assert!(measured.trace.live.iter().any(|bytes| *bytes > 0));
        data.release_scratch_v1(&mut measured);
        assert_eq!(measured.trace.live, [0; 3]);

        let mut stopped = admission(Some(measured.checkpoints));
        let mut cancelled = NormalizationDataV1::new_v1();
        assert_eq!(
            is_nfc_unbounded_into_v1(source, &mut cancelled, &mut stopped),
            Err(NormalizationDataRefusalV1::OperationRefused)
        );
        assert_eq!(
            cancelled.result_v1(),
            Some(Err(&NfcError::Admission("before canonical verdict")))
        );
        assert_eq!(stopped.trace.work, measured.trace.work);
        assert!(stopped.trace.live.iter().any(|bytes| *bytes > 0));
        assert!(stopped.trace.released.is_empty());
        cancelled.release_scratch_v1(&mut stopped);
        assert_eq!(stopped.trace.live, [0; 3]);
    }
}

#[test]
fn prepaid_borrowed_comparison_keeps_stop_and_capacity_distinct_v1() {
    let canonical = format!("x{}B", "\u{301}".repeat(1600));
    let different = format!("A{}B", "\u{315}\u{300}\u{301}".repeat(535));
    for source in [&canonical, &different] {
        let expected = source.chars().eq(source.nfc());
        let reserve = 40 * source.nfd().count();
        assert_eq!(
            try_is_nfc_with_prepaid_scratch_v1(source, reserve, || Ok::<(), &str>(())),
            Ok(expected)
        );
        assert_eq!(
            try_is_nfc_with_prepaid_scratch_v1(source, 0, || Ok::<(), &str>(())),
            Err(PrepaidNfcError::ScratchLimitExceeded)
        );
        let mut polls = 0;
        assert_eq!(
            try_is_nfc_with_prepaid_scratch_v1(source, reserve, || {
                polls += 1;
                if polls == 300 {
                    return Err("exact request refusal");
                }
                Ok(())
            }),
            Err(PrepaidNfcError::Normalization(NfcError::Admission(
                "exact request refusal"
            )))
        );
        assert_eq!(polls, 300);
    }
}
