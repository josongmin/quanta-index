// SPDX-License-Identifier: Apache-2.0

use ciborium::{de::from_reader, ser::into_writer, tag::*, value::Value};
use rstest::rstest;
use serde::{de::DeserializeOwned, Serialize};

use core::fmt::Debug;

#[rstest(item, bytes, value, encode, success,
    case(Captured(Some(6), true), "c6f5", Value::Tag(6, Value::Bool(true).into()), true, true),
    case(Captured(None, true), "f5", Value::Bool(true), true, true),

    case(Required::<_, 6>(true), "c6f5", Value::Tag(6, Value::Bool(true).into()), true, true),
    case(Required::<_, 6>(true), "c7f5", Value::Tag(7, Value::Bool(true).into()), false, false),
    case(Required::<_, 6>(true), "f5", Value::Bool(true), false, false),

    case(Accepted::<_, 6>(true), "c6f5", Value::Tag(6, Value::Bool(true).into()), true, true),
    case(Accepted::<_, 6>(true), "c7f5", Value::Tag(7, Value::Bool(true).into()), false, false),
    case(Accepted::<_, 6>(true), "f5", Value::Bool(true), false, true),
)]
fn test<T: Serialize + DeserializeOwned + Debug + Eq>(
    item: T,
    bytes: &str,
    value: Value,
    encode: bool,
    success: bool,
) {
    test_case(item, bytes, value, encode, success);
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the generated rstest fixtures supply owned tag items and values to this assertion helper"
)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "this fixed-gold assertion helper must fail on invalid fixture bytes, failed roundtrips or an unexpected tag result"
)]
fn test_case<T: Serialize + DeserializeOwned + Debug + Eq>(
    item: T,
    bytes: &str,
    value: Value,
    encode: bool,
    success: bool,
) {
    let bytes = hex::decode(bytes).unwrap();

    if encode {
        // Encode into bytes
        let mut encoded = Vec::new();
        into_writer(&item, &mut encoded).unwrap();
        assert_eq!(bytes, encoded);

        // Encode into value
        assert_eq!(value, Value::serialized(&item).unwrap());
    }

    // Decode from bytes
    match from_reader(&bytes[..]) {
        Ok(x) if success => assert_eq!(item, x),
        Ok(..) => panic!("unexpected success"),
        Err(error) if success => panic!("expected CBOR byte decode: {error:?}"),
        Err(..) => (),
    }

    // Decode from value
    match value.deserialized() {
        Ok(x) if success => assert_eq!(item, x),
        Ok(..) => panic!("unexpected success"),
        Err(error) if success => panic!("expected CBOR value decode: {error:?}"),
        Err(..) => (),
    }
}
