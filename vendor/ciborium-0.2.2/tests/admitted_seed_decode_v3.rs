use ciborium::de::{
    from_slice_with_finite_seed_and_allocation_v3, FiniteParserAllocationAdmissionV3,
    FiniteParserAllocationRefusalV3,
};
use serde::de::{DeserializeSeed, Deserializer};
use serde::Deserialize;
use std::marker::PhantomData;

struct TypedSeed<T>(PhantomData<T>);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DecodePhase {
    #[default]
    Fresh,
    Preflighting,
    PreflightReady,
    Begun,
    Completed,
    AbortedBeforeDecode,
    AbortedAfterDecode,
    AbortedAfterCompletion,
}

impl DecodePhase {
    fn is_begun(self) -> bool {
        matches!(
            self,
            Self::Begun | Self::Completed | Self::AbortedAfterDecode | Self::AbortedAfterCompletion
        )
    }

    fn is_aborted(self) -> bool {
        matches!(
            self,
            Self::AbortedBeforeDecode | Self::AbortedAfterDecode | Self::AbortedAfterCompletion
        )
    }

    fn is_completed(self) -> bool {
        matches!(self, Self::Completed | Self::AbortedAfterCompletion)
    }
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for TypedSeed<T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        T::deserialize(decoder)
    }
}

#[derive(Default)]
struct AppendOwner {
    calls: usize,
    refuse_after: Option<usize>,
    text_bytes: usize,
    binary_bytes: usize,
    phase: DecodePhase,
    max_text_length: Option<usize>,
    max_bytes_length: Option<usize>,
    max_sequence_length: Option<usize>,
    text_headers: usize,
    bytes_headers: usize,
    sequence_headers: usize,
}

impl AppendOwner {
    fn before_append(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        if self.phase != DecodePhase::Begun
            || self.refuse_after.is_some_and(|limit| self.calls >= limit)
        {
            return Err(FiniteParserAllocationRefusalV3);
        }
        self.calls = self
            .calls
            .checked_add(1)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        Ok(())
    }
}

impl FiniteParserAllocationAdmissionV3 for AppendOwner {
    fn begin_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        if self.phase != DecodePhase::Fresh {
            return Err(FiniteParserAllocationRefusalV3);
        }
        self.phase = DecodePhase::Preflighting;
        Ok(())
    }

    fn complete_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        if self.phase != DecodePhase::Preflighting {
            return Err(FiniteParserAllocationRefusalV3);
        }
        self.phase = DecodePhase::PreflightReady;
        Ok(())
    }

    fn begin_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        if !matches!(self.phase, DecodePhase::Fresh | DecodePhase::PreflightReady) {
            return Err(FiniteParserAllocationRefusalV3);
        }
        self.phase = DecodePhase::Begun;
        Ok(())
    }

    fn complete_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        if self.phase != DecodePhase::Begun {
            return Err(FiniteParserAllocationRefusalV3);
        }
        self.phase = DecodePhase::Completed;
        Ok(())
    }

    fn abort_decode_v3(&mut self) {
        self.phase = match self.phase {
            DecodePhase::Begun => DecodePhase::AbortedAfterDecode,
            DecodePhase::Completed => DecodePhase::AbortedAfterCompletion,
            DecodePhase::Fresh | DecodePhase::Preflighting | DecodePhase::PreflightReady => {
                DecodePhase::AbortedBeforeDecode
            }
            aborted @ (DecodePhase::AbortedBeforeDecode
            | DecodePhase::AbortedAfterDecode
            | DecodePhase::AbortedAfterCompletion) => aborted,
        };
    }

    fn check_text_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.before_header(length, self.max_text_length)?;
        self.text_headers = self
            .text_headers
            .checked_add(1)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        Ok(())
    }

    fn check_bytes_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.before_header(length, self.max_bytes_length)?;
        self.bytes_headers = self
            .bytes_headers
            .checked_add(1)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        Ok(())
    }

    fn check_sequence_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.before_header(length, self.max_sequence_length)?;
        self.sequence_headers = self
            .sequence_headers
            .checked_add(1)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        Ok(())
    }

    fn reserve_text_v3(
        &mut self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        let next_text_bytes = self
            .text_bytes
            .checked_add(additional)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        self.before_append()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| FiniteParserAllocationRefusalV3)?;
        self.text_bytes = next_text_bytes;
        Ok(())
    }

    fn reserve_bytes_v3(
        &mut self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        let next_binary_bytes = self
            .binary_bytes
            .checked_add(additional)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        self.before_append()?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_allocation_error| FiniteParserAllocationRefusalV3)?;
        self.binary_bytes = next_binary_bytes;
        Ok(())
    }
}

impl AppendOwner {
    fn before_header(
        &self,
        length: Option<usize>,
        limit: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        if self.phase != DecodePhase::Begun
            || length
                .zip(limit)
                .is_some_and(|(length, limit)| length > limit)
        {
            return Err(FiniteParserAllocationRefusalV3);
        }
        Ok(())
    }
}

#[test]
fn controlled_text_refuses_before_second_parser_append() {
    let text = "x".repeat(5000);
    let mut wire = Vec::new();
    assert!(ciborium::ser::into_writer(&text, &mut wire).is_ok());
    let mut scratch = [0u8; 4096];
    let mut owner = AppendOwner {
        refuse_after: Some(1),
        ..AppendOwner::default()
    };
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &wire,
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert_eq!(owner.calls, 1);
    assert!(owner.text_bytes > 0 && owner.text_bytes < 5000);
    assert!(owner.phase.is_aborted());
}

#[test]
fn controlled_byte_string_uses_same_parser_and_typed_seed() {
    let mut wire = vec![0x59, 0x13, 0x88]; // Definite 5000-byte CBOR byte string.
    wire.extend(std::iter::repeat_n(0x7f, 5000));
    let mut scratch = [0u8; 4096];
    let mut owner = AppendOwner::default();
    let decoded = from_slice_with_finite_seed_and_allocation_v3(
        &wire,
        &mut scratch,
        TypedSeed::<Vec<u8>>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    match decoded {
        Ok(value) => assert_eq!(value, vec![0x7f; 5000]),
        Err(error) => panic!("controlled CBOR byte string failed: {error:?}"),
    }
    assert_eq!(owner.binary_bytes, 5000);
    assert!(owner.calls >= 2);
    assert!(!owner.phase.is_aborted());
}

#[test]
fn malformed_input_refuses_without_parser_allocation() {
    let mut scratch = [0u8; 4096];
    let mut owner = AppendOwner::default();
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x5a, 0xff, 0xff, 0xff, 0xff],
        &mut scratch,
        TypedSeed::<Vec<u8>>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert_eq!(owner.calls, 0);
    assert!(owner.phase.is_aborted());
}

#[test]
fn a_noop_admission_cannot_trigger_implicit_parser_growth() {
    struct Noop;
    impl FiniteParserAllocationAdmissionV3 for Noop {
        fn begin_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn complete_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn begin_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn complete_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn abort_decode_v3(&mut self) {}

        fn check_text_length_v3(
            &mut self,
            _: Option<usize>,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn check_bytes_length_v3(
            &mut self,
            _: Option<usize>,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn check_sequence_length_v3(
            &mut self,
            _: Option<usize>,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn reserve_text_v3(
            &mut self,
            _: &mut String,
            _: usize,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }

        fn reserve_bytes_v3(
            &mut self,
            _: &mut Vec<u8>,
            _: usize,
        ) -> Result<(), FiniteParserAllocationRefusalV3> {
            Ok(())
        }
    }

    let mut scratch = [0u8; 4];
    let text = from_slice_with_finite_seed_and_allocation_v3(
        &[0x65, b'h', b'e', b'l', b'l', b'o'],
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut Noop,
    );
    assert!(text.is_err());

    let bytes = from_slice_with_finite_seed_and_allocation_v3(
        &[0x45, 1, 2, 3, 4, 5],
        &mut scratch,
        TypedSeed::<Vec<u8>>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut Noop,
    );
    assert!(bytes.is_err());
}

#[test]
fn original_work_refusal_precedes_parser_allocation() {
    #[derive(Debug, PartialEq)]
    struct WorkRefused;

    let mut scratch = [0u8; 4];
    let mut owner = AppendOwner::default();
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x65, b'h', b'e', b'l', b'l', b'o'],
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Err(WorkRefused),
        &mut owner,
    );
    assert!(matches!(
        result,
        Err(ciborium::de::Error::Io(
            ciborium::de::FiniteReadErrorV3::Work(WorkRefused)
        ))
    ));
    assert_eq!(owner.calls, 0);
}

#[test]
fn truncated_text_after_first_append_ends_the_shared_attempt() {
    let mut scratch = [0_u8; 4];
    let mut owner = AppendOwner::default();
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x69, b'a', b'l', b'p', b'h', b'a'],
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert!(owner.text_bytes > 0);
    assert!(owner.phase.is_aborted());
    let charged = owner.text_bytes;
    let mut fresh = String::new();
    assert!(owner.reserve_text_v3(&mut fresh, 1).is_err());
    assert_eq!(owner.text_bytes, charged);
    assert_eq!(fresh.capacity(), 0);
}

#[test]
fn a_typed_seed_refusal_after_parser_birth_is_fatal() {
    struct RefuseOwnedText;
    impl<'de> DeserializeSeed<'de> for RefuseOwnedText {
        type Value = String;
        fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<String, D::Error> {
            let _text = String::deserialize(decoder)?;
            Err(serde::de::Error::custom("fixed typed rejection"))
        }
    }
    let mut scratch = [0_u8; 4];
    let mut owner = AppendOwner::default();
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x65, b'a', b'l', b'p', b'h', b'a'],
        &mut scratch,
        RefuseOwnedText,
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert_eq!(owner.text_bytes, 5);
    assert!(owner.phase.is_aborted());
}

#[test]
fn final_work_refusal_keeps_the_original_cause_and_aborts() {
    struct MarkDecodedText<'a>(&'a std::cell::Cell<bool>);
    impl<'de> DeserializeSeed<'de> for MarkDecodedText<'_> {
        type Value = String;
        fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<String, D::Error> {
            let value = String::deserialize(decoder)?;
            self.0.set(true);
            Ok(value)
        }
    }
    let decoded = std::cell::Cell::new(false);
    let mut scratch = [0_u8; 4];
    let mut owner = AppendOwner::default();
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x65, b'a', b'l', b'p', b'h', b'a'],
        &mut scratch,
        MarkDecodedText(&decoded),
        &mut |bytes| {
            if bytes == 0 && decoded.get() {
                return Err("final work refused");
            }
            Ok(())
        },
        &mut owner,
    );
    assert!(matches!(
        result,
        Err(ciborium::de::Error::Io(
            ciborium::de::FiniteReadErrorV3::Work("final work refused")
        ))
    ));
    assert_eq!(owner.text_bytes, 5);
    assert!(decoded.get());
    assert!(owner.phase.is_aborted());
}

#[test]
fn scratch_refusal_also_ends_the_attempt_before_birth() {
    let mut scratch = [0_u8; 3];
    let mut owner = AppendOwner::default();
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x61, b'a'],
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert_eq!(owner.calls, 0);
    assert!(owner.phase.is_aborted());
}

#[test]
fn definite_text_header_refuses_before_first_parser_reserve() {
    let mut scratch = [0_u8; 4];
    let mut owner = AppendOwner {
        max_text_length: Some(4),
        ..AppendOwner::default()
    };
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x65, b'h', b'e', b'l', b'l', b'o'],
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert!(owner.phase.is_begun() && owner.phase.is_aborted() && !owner.phase.is_completed());
    assert_eq!(owner.calls, 0);
    assert_eq!(owner.text_bytes, 0);
    assert_eq!(owner.text_headers, 0);
}

#[test]
fn definite_byte_header_refuses_before_parser_reserve() {
    let mut scratch = [0_u8; 4];
    let mut owner = AppendOwner {
        max_bytes_length: Some(4),
        ..AppendOwner::default()
    };
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x45, 1, 2, 3, 4, 5],
        &mut scratch,
        TypedSeed::<Vec<u8>>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert_eq!(owner.calls, 0);
    assert_eq!(owner.binary_bytes, 0);
    assert!(owner.phase.is_aborted());
}

#[test]
fn borrowed_bytes_route_checks_header_before_visiting() {
    struct ByteSeed<'a>(&'a std::cell::Cell<bool>);
    impl<'de> DeserializeSeed<'de> for ByteSeed<'_> {
        type Value = usize;
        fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<usize, D::Error> {
            struct ByteVisitor<'a>(&'a std::cell::Cell<bool>);
            impl serde::de::Visitor<'_> for ByteVisitor<'_> {
                type Value = usize;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("bytes")
                }
                fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<usize, E> {
                    self.0.set(true);
                    Ok(bytes.len())
                }
            }
            decoder.deserialize_bytes(ByteVisitor(self.0))
        }
    }
    let visited = std::cell::Cell::new(false);
    let mut scratch = [0_u8; 8];
    let mut owner = AppendOwner {
        max_bytes_length: Some(4),
        ..AppendOwner::default()
    };
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x45, 1, 2, 3, 4, 5],
        &mut scratch,
        ByteSeed(&visited),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert!(!visited.get());
    assert_eq!(owner.calls, 0);
    assert!(owner.phase.is_aborted());
}

#[test]
fn definite_array_count_refuses_before_sequence_visitor() {
    let mut scratch = [0_u8; 8];
    let mut owner = AppendOwner {
        max_sequence_length: Some(4),
        ..AppendOwner::default()
    };
    let result = from_slice_with_finite_seed_and_allocation_v3(
        &[0x85, 1, 2, 3, 4, 5],
        &mut scratch,
        TypedSeed::<Vec<u8>>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(result.is_err());
    assert_eq!(owner.sequence_headers, 0);
    assert_eq!(owner.calls, 0);
    assert!(owner.phase.is_aborted());
}

#[test]
fn completed_or_aborted_attempt_cannot_start_again() {
    let mut scratch = [0_u8; 4];
    let mut owner = AppendOwner::default();
    let first = from_slice_with_finite_seed_and_allocation_v3(
        &[0x61, b'a'],
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    match first {
        Ok(value) => assert_eq!(value, "a"),
        Err(error) => panic!("first admitted decode failed: {error:?}"),
    }
    assert!(owner.phase.is_completed());
    let calls = owner.calls;
    let second = from_slice_with_finite_seed_and_allocation_v3(
        &[0x61, b'b'],
        &mut scratch,
        TypedSeed::<String>(PhantomData),
        &mut |_| Ok::<(), ()>(()),
        &mut owner,
    );
    assert!(second.is_err());
    assert!(owner.phase.is_aborted());
    assert_eq!(owner.calls, calls);
}

#[test]
fn unwinding_seed_aborts_decode_attempt() {
    struct PanicSeed;
    impl<'de> DeserializeSeed<'de> for PanicSeed {
        type Value = String;
        #[expect(
            clippy::panic_in_result_fn,
            reason = "this seed deliberately unwinds after parser allocation to test attempt poisoning"
        )]
        fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<String, D::Error> {
            drop(String::deserialize(decoder)?);
            panic!("fixed test unwind after parser allocation");
        }
    }
    let mut scratch = [0_u8; 4];
    let mut owner = AppendOwner::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(from_slice_with_finite_seed_and_allocation_v3(
            &[0x61, b'a'],
            &mut scratch,
            PanicSeed,
            &mut |_| Ok::<(), ()>(()),
            &mut owner,
        ));
    }));
    assert!(result.is_err());
    assert!(owner.phase.is_aborted());
    assert_eq!(owner.text_bytes, 1);
    assert!(owner.reserve_text_v3(&mut String::new(), 1).is_err());
}
