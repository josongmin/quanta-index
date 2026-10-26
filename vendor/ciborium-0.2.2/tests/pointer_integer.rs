// SPDX-License-Identifier: Apache-2.0

use ciborium::value::Integer;

#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
#[test]
fn pointer_integer_extremes_preserve_value_and_range() {
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
