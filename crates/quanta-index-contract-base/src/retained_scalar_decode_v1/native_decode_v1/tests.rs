use super::*;
use core::fmt;
use serde::de::{Expected, SeqAccess, Unexpected, Visitor};
use std::cell::Cell;

#[derive(Debug, Eq, PartialEq)]
enum ErrorKind {
    Type,
    Value,
    Length,
    Custom,
    Original,
}

#[derive(Debug)]
struct FullError {
    kind: ErrorKind,
    original: Box<u8>,
}
impl FullError {
    fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            original: Box::new(71),
        }
    }
}
impl fmt::Display for FullError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("finite test error")
    }
}
impl std::error::Error for FullError {}
impl de::Error for FullError {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self::new(ErrorKind::Custom)
    }
    fn invalid_type(_: Unexpected<'_>, _: &dyn Expected) -> Self {
        Self::new(ErrorKind::Type)
    }
    fn invalid_value(_: Unexpected<'_>, _: &dyn Expected) -> Self {
        Self::new(ErrorKind::Value)
    }
    fn invalid_length(_: usize, _: &dyn Expected) -> Self {
        Self::new(ErrorKind::Length)
    }
}

enum Input {
    Unsigned(u64),
    Signed(i64),
    String(String),
    Bytes(Vec<u8>),
    Failure(FullError),
}
struct InputDeserializer<'a> {
    input: Input,
    polls: &'a Cell<usize>,
}
impl<'de> Deserializer<'de> for InputDeserializer<'_> {
    type Error = FullError;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FullError> {
        self.polls
            .set(self.polls.get().checked_add(1).expect("test poll count"));
        match self.input {
            Input::Unsigned(value) => visitor.visit_u64(value),
            Input::Signed(value) => visitor.visit_i64(value),
            Input::String(value) => visitor.visit_string(value),
            Input::Bytes(value) => visitor.visit_byte_buf(value),
            Input::Failure(cause) => Err(cause),
        }
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

struct ArrayDeserializer<'a> {
    elements: &'a mut [Option<Input>],
    polls: &'a Cell<usize>,
}
impl<'de> Deserializer<'de> for ArrayDeserializer<'_> {
    type Error = FullError;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FullError> {
        visitor.visit_seq(self)
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}
impl<'de> SeqAccess<'de> for ArrayDeserializer<'_> {
    type Error = FullError;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, FullError> {
        let Some((first, rest)) = core::mem::take(&mut self.elements).split_first_mut() else {
            return Ok(None);
        };
        self.elements = rest;
        let input = first.take().expect("test input occurrence");
        seed.deserialize(InputDeserializer {
            input,
            polls: self.polls,
        })
        .map(Some)
    }
}

fn decode<T: NativeRetainedScalarLeafV1>(
    input: Input,
    data: &mut NativeRetainedScalarDecodeDataV1<T>,
    failure: &mut Option<FullError>,
    polls: &Cell<usize>,
) -> Result<(), Refusal> {
    data.try_decode_into_v1(InputDeserializer { input, polls }, failure)
}

#[test]
fn canonical_integer_boundaries_and_errors_v1() {
    for value in [0, 1, u64::MAX] {
        let mut data = NativeRetainedScalarDecodeDataV1::<u64>::new_v1();
        decode(Input::Unsigned(value), &mut data, &mut None, &Cell::new(0)).unwrap();
        let mut output = None;
        data.complete_into_slot_v1(&mut output).unwrap();
        assert_eq!(output, Some(value));
    }
    for value in [0, 1, u64::from(u32::MAX)] {
        let mut data = NativeRetainedScalarDecodeDataV1::<u32>::new_v1();
        decode(Input::Unsigned(value), &mut data, &mut None, &Cell::new(0)).unwrap();
        let mut output = None;
        data.complete_into_slot_v1(&mut output).unwrap();
        assert_eq!(output.map(u64::from), Some(value));
    }
    for input in [Input::Signed(-1), Input::Unsigned(u64::from(u32::MAX) + 1)] {
        let mut data = NativeRetainedScalarDecodeDataV1::<u32>::new_v1();
        let mut failure = None;
        assert_eq!(
            decode(input, &mut data, &mut failure, &Cell::new(0)),
            Err(Refusal::OperationRefused)
        );
        assert_eq!(failure.as_ref().unwrap().kind, ErrorKind::Value);
        assert_eq!(
            data.complete_into_slot_v1(&mut None),
            Err(Refusal::MissingResult)
        );
    }
}

fn assert_owned_wrong_type<T: NativeRetainedScalarLeafV1>() {
    let wire = String::from("owned wrong scalar");
    let pointer = wire.as_ptr();
    let mut data = NativeRetainedScalarDecodeDataV1::<T>::new_v1();
    let mut failure = None;
    let polls = Cell::new(0);
    assert_eq!(
        decode(Input::String(wire), &mut data, &mut failure, &polls),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(data.retained_string_v1().unwrap().as_ptr(), pointer);
    assert_eq!(data.retained_string_v1(), Some("owned wrong scalar"));
    assert_eq!(failure.as_ref().unwrap().kind, ErrorKind::Type);
    let cause_pointer = core::ptr::from_ref(failure.as_ref().unwrap().original.as_ref());
    assert_eq!(
        decode(Input::Unsigned(9), &mut data, &mut failure, &polls),
        Err(Refusal::OccupiedOutput)
    );
    assert_eq!(
        core::ptr::from_ref(failure.as_ref().unwrap().original.as_ref()),
        cause_pointer
    );
    let mut second_failure = None;
    assert_eq!(
        decode(Input::Unsigned(10), &mut data, &mut second_failure, &polls),
        Err(Refusal::UsedData)
    );
    assert_eq!(polls.get(), 1);
    assert!(second_failure.is_none());
    assert_eq!(data.retained_string_v1().unwrap().as_ptr(), pointer);

    let wire = vec![2, 5, 8];
    let pointer = wire.as_ptr();
    let mut data = NativeRetainedScalarDecodeDataV1::<T>::new_v1();
    assert_eq!(
        decode(Input::Bytes(wire), &mut data, &mut None, &Cell::new(0)),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(data.retained_bytes_v1(), Some([2, 5, 8].as_slice()));
    assert_eq!(data.retained_bytes_v1().unwrap().as_ptr(), pointer);
}

#[test]
fn closed_leaves_retain_owned_string_and_bytes_without_copy_v1() {
    assert_owned_wrong_type::<u32>();
    assert_owned_wrong_type::<u64>();
    assert_owned_wrong_type::<[u8; 32]>();
}

#[test]
fn byte_array_stops_before_second_owned_input_v1() {
    for wrong_index in [0, 7, 31] {
        for bytes in [false, true] {
            let wrong = if bytes {
                Input::Bytes(vec![7, 11, 17])
            } else {
                Input::String(String::from("first owned input"))
            };
            let first_pointer = match &wrong {
                Input::String(value) => value.as_ptr(),
                Input::Bytes(value) => value.as_ptr(),
                Input::Unsigned(_) | Input::Signed(_) | Input::Failure(_) => {
                    panic!("expected owned test input");
                }
            };
            let second = String::from("unpolled second owned input");
            let second_pointer = second.as_ptr();
            let mut elements: Vec<_> = (0..wrong_index)
                .map(|_| Some(Input::Unsigned(1)))
                .chain([Some(wrong), Some(Input::String(second))])
                .collect();
            let polls = Cell::new(0);
            let mut data = NativeRetainedScalarDecodeDataV1::<[u8; 32]>::new_v1();
            let mut failure = None;
            assert_eq!(
                data.try_decode_into_v1(
                    ArrayDeserializer {
                        elements: &mut elements,
                        polls: &polls
                    },
                    &mut failure
                ),
                Err(Refusal::OperationRefused)
            );
            assert_eq!(polls.get(), wrong_index + 1);
            assert_eq!(failure.as_ref().unwrap().kind, ErrorKind::Type);
            match elements.get(wrong_index + 1).unwrap().as_ref().unwrap() {
                Input::String(value) => assert_eq!(value.as_ptr(), second_pointer),
                Input::Unsigned(_) | Input::Signed(_) | Input::Bytes(_) | Input::Failure(_) => {
                    panic!("second owned input was changed");
                }
            }
            assert_eq!(data.retained_bytes_v1().is_some(), bytes);
            assert_eq!(data.retained_string_v1().is_some(), !bytes);
            let retained_pointer = if bytes {
                data.retained_bytes_v1().unwrap().as_ptr()
            } else {
                data.retained_string_v1().unwrap().as_ptr()
            };
            assert_eq!(retained_pointer, first_pointer);
        }
    }
}

#[test]
fn byte_array_uses_the_existing_u8_and_length_predicates_v1() {
    for length in [0, 31, 32] {
        let mut elements: Vec<_> = (0..length).map(|_| Some(Input::Unsigned(255))).collect();
        let mut data = NativeRetainedScalarDecodeDataV1::<[u8; 32]>::new_v1();
        let mut failure = None;
        let polls = Cell::new(0);
        let result = data.try_decode_into_v1(
            ArrayDeserializer {
                elements: &mut elements,
                polls: &polls,
            },
            &mut failure,
        );
        if length == 32 {
            result.unwrap();
            let mut output = None;
            data.complete_into_slot_v1(&mut output).unwrap();
            assert_eq!(output, Some([255; 32]));
        } else {
            assert_eq!(result, Err(Refusal::OperationRefused));
            assert_eq!(failure.as_ref().unwrap().kind, ErrorKind::Length);
        }
        assert_eq!(polls.get(), length);
    }
    let mut elements = [Some(Input::Unsigned(256))];
    let mut data = NativeRetainedScalarDecodeDataV1::<[u8; 32]>::new_v1();
    let mut failure = None;
    assert_eq!(
        data.try_decode_into_v1(
            ArrayDeserializer {
                elements: &mut elements,
                polls: &Cell::new(0)
            },
            &mut failure
        ),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(failure.as_ref().unwrap().kind, ErrorKind::Value);
}

#[test]
fn complete_original_error_and_occupied_slots_survive_without_polls_v1() {
    let cause = FullError::new(ErrorKind::Original);
    let pointer = core::ptr::from_ref(cause.original.as_ref());
    let polls = Cell::new(0);
    let mut data = NativeRetainedScalarDecodeDataV1::<u64>::new_v1();
    let mut failure = None;
    assert_eq!(
        decode(Input::Failure(cause), &mut data, &mut failure, &polls),
        Err(Refusal::OperationRefused)
    );
    assert_eq!(
        core::ptr::from_ref(failure.as_ref().unwrap().original.as_ref()),
        pointer
    );
    assert_eq!(failure.as_ref().unwrap().kind, ErrorKind::Original);
    assert_eq!(
        decode(Input::Unsigned(3), &mut data, &mut failure, &polls),
        Err(Refusal::OccupiedOutput)
    );
    assert_eq!(polls.get(), 1);
    let mut fresh = NativeRetainedScalarDecodeDataV1::<u64>::new_v1();
    assert_eq!(
        decode(Input::Unsigned(4), &mut fresh, &mut failure, &polls),
        Err(Refusal::OccupiedOutput)
    );
    assert!(fresh.is_fresh_v1());
    assert_eq!(polls.get(), 1);
}

#[test]
fn unit_seed_and_pure_transfer_do_not_reset_one_attempt_v1() {
    let polls = Cell::new(0);
    let mut data = NativeRetainedScalarDecodeDataV1::<u64>::new_v1();
    data.native_decode_seed_v1()
        .deserialize(InputDeserializer {
            input: Input::Unsigned(9),
            polls: &polls,
        })
        .unwrap();
    let mut output = Some(41);
    assert_eq!(
        data.complete_into_slot_v1(&mut output),
        Err(Refusal::OccupiedOutput)
    );
    assert_eq!(output, Some(41));
    output = None;
    data.complete_into_slot_v1(&mut output).unwrap();
    assert_eq!(output, Some(9));
    assert_eq!(
        data.complete_into_slot_v1(&mut None),
        Err(Refusal::MissingResult)
    );
    let failure = data
        .native_decode_seed_v1()
        .deserialize(InputDeserializer {
            input: Input::Unsigned(10),
            polls: &polls,
        })
        .unwrap_err();
    assert_eq!(failure.kind, ErrorKind::Custom);
    assert_eq!(polls.get(), 1);
    assert!(!data.is_fresh_v1());
}
