//! Product-owned CBOR wire contracts retained when the codec comes from crates.io.

use ciborium::value::{CanonicalValue, Value};

#[expect(
    clippy::expect_used,
    reason = "a fixed wire oracle must fail if encoding fails"
)]
fn encode<T: serde::Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).expect("fixed CBOR fixture must encode");
    bytes
}

#[test]
fn imported_macro_keeps_nested_null_wire() {
    use ciborium::cbor;

    let value = cbor!([null, {null => [null]}]).expect("fixed nested null value");
    assert_eq!(encode(&value), [0x82, 0xf6, 0xa1, 0xf6, 0x81, 0xf6]);
}

#[test]
fn canonical_key_order_keeps_persisted_map_wire() {
    let mut keys = std::collections::BTreeMap::new();
    for (key, value) in [
        (Value::Text("aa".to_owned()), 6_u8),
        (Value::Integer(100.into()), 3),
        (Value::Bool(false), 2),
        (Value::Array(vec![Value::Integer((-1).into())]), 5),
        (Value::Integer((-1).into()), 1),
        (Value::Text("z".to_owned()), 4),
        (Value::Integer(10.into()), 0),
        (Value::Array(vec![Value::Integer(100.into())]), 7),
    ] {
        let previous = keys.insert(CanonicalValue::from(key), value);
        assert!(previous.is_none());
    }
    let bytes = encode(&keys);
    assert_eq!(
        bytes,
        [
            0xa8, 0x0a, 0x00, 0x20, 0x01, 0xf4, 0x02, 0x18, 0x64, 0x03, 0x61, b'z', 0x04, 0x81,
            0x20, 0x05, 0x62, b'a', b'a', 0x06, 0x81, 0x18, 0x64, 0x07,
        ]
    );
    let decoded: Value = ciborium::de::from_reader(bytes.as_slice()).expect("canonical map");
    assert!(matches!(decoded, Value::Map(fields) if fields.len() == 8));
}

#[test]
fn integer_and_float_boundaries_keep_exact_wire() {
    assert_eq!(
        encode(&u64::MAX),
        [0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
    );
    assert_eq!(
        encode(&(u128::from(u64::MAX).saturating_add(1))),
        [0xc2, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        encode(&(-18_446_744_073_709_551_617_i128)),
        [0xc3, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(encode(&1.0_f64), [0xf9, 0x3c, 0x00]);
    assert_eq!(encode(&-0.0_f64), [0xf9, 0x80, 0x00]);
    assert_eq!(encode(&f32::MAX), [0xfa, 0x7f, 0x7f, 0xff, 0xff]);
    assert_eq!(encode(&f64::INFINITY), [0xf9, 0x7c, 0x00]);
    assert_eq!(encode(&Value::Float(f64::NAN)), [0xf9, 0x7e, 0x00]);
}

#[test]
fn required_tag_and_dynamic_value_keep_fixed_wire() {
    use ciborium::tag::Required;

    let tagged = Required::<u64, 42>(7);
    let bytes = [0xd8, 0x2a, 0x07];
    assert_eq!(encode(&tagged), bytes);
    assert_eq!(
        encode(&Value::Tag(42, Box::new(Value::Integer(7.into())))),
        bytes
    );
    let decoded: Required<u64, 42> =
        ciborium::de::from_reader(bytes.as_slice()).expect("fixed tag");
    assert_eq!(decoded, tagged);
    let wrong: Result<Required<u64, 43>, _> = ciborium::de::from_reader(bytes.as_slice());
    assert!(wrong.is_err());
}

#[test]
fn malformed_text_and_integer_keep_syntax_offsets() {
    use ciborium::de::Error;

    for (wire, offset) in [
        (&[0x1e][..], 0),
        (&[0x1f][..], 0),
        (&[0x83, 0x01, 0x1f, 0x03][..], 2),
        (&[0x7f, 0x61, b'a', 0x01, 0xff][..], 3),
        (&[0x62, 0xc3, 0x28][..], 0),
        (&[0x7f, 0x62, 0xc3, 0x28, 0xff][..], 1),
    ] {
        let decoded: Result<Value, _> = ciborium::de::from_reader(wire);
        assert!(matches!(decoded, Err(Error::Syntax(actual)) if actual == offset));
    }
}

#[test]
fn long_utf8_and_dynamic_bytes_round_trip() {
    let text = "ボ".repeat(2_000);
    let text_wire = encode(&text);
    let decoded: String = ciborium::de::from_reader(text_wire.as_slice()).expect("long UTF-8");
    assert_eq!(decoded, text);

    let value = Value::Bytes(vec![0, 0xff, 0x80, 1]);
    let bytes = encode(&value);
    assert_eq!(bytes, [0x44, 0, 0xff, 0x80, 1]);
    let decoded: Value = ciborium::de::from_reader(bytes.as_slice()).expect("dynamic bytes");
    assert_eq!(decoded, value);
}

#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
#[test]
fn pointer_integer_bounds_keep_conversion_semantics() {
    use ciborium::value::Integer;

    for value in [0usize, 1, usize::MAX] {
        let integer = Integer::from(value);
        assert_eq!(usize::try_from(integer), Ok(value));
        assert_eq!(u128::try_from(integer), u128::try_from(value));
    }
    for value in [isize::MIN, -1, 0, 1, isize::MAX] {
        let integer = Integer::from(value);
        assert_eq!(isize::try_from(integer), Ok(value));
        assert_eq!(i128::try_from(value), Ok(i128::from(integer)));
    }
    assert!(usize::try_from(Integer::from(-1i8)).is_err());
    assert!(isize::try_from(Integer::from(usize::MAX)).is_err());
}

#[test]
fn recursion_limit_separates_nested_containers_from_incomplete_scalars() {
    use ciborium::de::{Error, from_reader_with_recursion_limit};

    for header in [0x9f, 0xbf] {
        let nested = [header; 33];
        for limit in [8, 16, 32] {
            let beyond = nested.get(..=limit).expect("bounded fixture");
            let within = nested.get(..limit).expect("bounded fixture");
            let deeper: Result<Value, _> = from_reader_with_recursion_limit(beyond, limit);
            let incomplete: Result<Value, _> = from_reader_with_recursion_limit(within, limit);
            assert!(matches!(deeper, Err(Error::RecursionLimitExceeded)));
            assert!(matches!(incomplete, Err(Error::Io(_))));
        }
    }
    for header in [0x5f, 0x7f] {
        let wire = [header; 33];
        let incomplete: Result<Value, _> = from_reader_with_recursion_limit(wire.as_slice(), 8);
        assert!(matches!(incomplete, Err(Error::Io(_))));
    }
}

#[test]
fn bounded_malformed_value_corpus_never_panics() {
    let mut state = 0xc1b0_12a5_2026_1013_u64;
    let safe_headers = [
        0x18, 0x41, 0x61, 0x81, 0xa1, 0x9f, 0xbf, 0x5f, 0x7f, 0xf4, 0xf5, 0xf6, 0xff,
    ];
    let mut rejected = 0_usize;
    for length in 0..32 {
        for _case in 0..4_096 {
            let mut bytes = Vec::with_capacity(length);
            for _byte in 0..length {
                state ^= state.wrapping_shl(13);
                state ^= state.wrapping_shr(7);
                state ^= state.wrapping_shl(17);
                bytes.push(u8::try_from(state & 0xff).expect("masked byte"));
            }
            if let Some(first) = bytes.first_mut() {
                let header_count = u64::try_from(safe_headers.len()).expect("fixed header count");
                let index = usize::try_from(state % header_count).expect("bounded index");
                *first = *safe_headers.get(index).expect("bounded header index");
            }
            let result = std::panic::catch_unwind(|| {
                ciborium::de::from_reader::<Value, _>(bytes.as_slice())
            });
            assert!(result.is_ok(), "parser panic on bounded fixture");
            if matches!(result, Ok(Err(_))) {
                rejected = rejected.saturating_add(1);
            }
        }
    }
    assert!(rejected > 0, "corpus must contain malformed CBOR");
}
