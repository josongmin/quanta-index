// SPDX-License-Identifier: Apache-2.0

//! This test validates that we don't get stack overflows.
//!
//! If container types cause recursion, then a long list of prefixes which
//! indicate nested container types could cause the stack to overflow. We
//! test each of these types here to ensure there is no stack overflow.

use ciborium::{
    de::{from_reader, from_reader_with_recursion_limit, Error},
    value::Value,
};

fn expect_recursion(error: &Error<std::io::Error>, limit: Option<usize>) {
    assert!(
        matches!(error, Error::RecursionLimitExceeded),
        "incorrect error with limit {limit:?}: {error:?}"
    );
}

fn expect_io(error: &Error<std::io::Error>, limit: Option<usize>) {
    assert!(
        matches!(error, Error::Io(_)),
        "incorrect error with limit {limit:?}: {error:?}"
    );
}

#[test]
fn array() {
    let bytes = vec![0x9f; 128 * 1024].into_boxed_slice();
    expect_recursion(&from_reader::<Value, _>(bytes.as_ref()).unwrap_err(), None);
}

#[test]
fn map() {
    let bytes = vec![0xbf; 128 * 1024].into_boxed_slice();
    expect_recursion(&from_reader::<Value, _>(bytes.as_ref()).unwrap_err(), None);
}

#[test]
fn bytes() {
    let bytes = vec![0x5f; 128 * 1024].into_boxed_slice();
    expect_io(&from_reader::<Value, _>(bytes.as_ref()).unwrap_err(), None);
}

#[test]
fn text() {
    let bytes = vec![0x7f; 128 * 1024].into_boxed_slice();
    expect_io(&from_reader::<Value, _>(bytes.as_ref()).unwrap_err(), None);
}

#[test]
fn array_limit() {
    let bytes = vec![0x9f; 128 * 1024].into_boxed_slice();
    for limit in 16..256 {
        expect_recursion(
            &from_reader_with_recursion_limit::<Value, _>(bytes.as_ref(), limit).unwrap_err(),
            Some(limit),
        );
        // Data nested beyond the limit must fail at the recursion boundary.
        let beyond = bytes
            .get(..=limit)
            .unwrap_or_else(|| panic!("array fixture shorter than limit {limit}"));
        expect_recursion(
            &from_reader_with_recursion_limit::<Value, _>(beyond, limit).unwrap_err(),
            Some(limit),
        );
        // Data nested within the limit is incomplete and must fail on input.
        let within = bytes
            .get(..limit)
            .unwrap_or_else(|| panic!("array fixture shorter than limit {limit}"));
        expect_io(
            &from_reader_with_recursion_limit::<Value, _>(within, limit).unwrap_err(),
            Some(limit),
        );
    }
}

#[test]
fn map_limit() {
    let bytes = vec![0xbf; 128 * 1024].into_boxed_slice();
    for limit in 16..256 {
        expect_recursion(
            &from_reader_with_recursion_limit::<Value, _>(bytes.as_ref(), limit).unwrap_err(),
            Some(limit),
        );
        // Data nested beyond the limit must fail at the recursion boundary.
        let beyond = bytes
            .get(..=limit)
            .unwrap_or_else(|| panic!("map fixture shorter than limit {limit}"));
        expect_recursion(
            &from_reader_with_recursion_limit::<Value, _>(beyond, limit).unwrap_err(),
            Some(limit),
        );
        // Data nested within the limit is incomplete and must fail on input.
        let within = bytes
            .get(..limit)
            .unwrap_or_else(|| panic!("map fixture shorter than limit {limit}"));
        expect_io(
            &from_reader_with_recursion_limit::<Value, _>(within, limit).unwrap_err(),
            Some(limit),
        );
    }
}
