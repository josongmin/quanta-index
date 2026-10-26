// SPDX-License-Identifier: Apache-2.0
use core::cmp::Ordering;

macro_rules! implfrom {
    ($( $(#[$($attr:meta)+])? $t:ident)+) => {
        $(
            $(#[$($attr)+])?
            impl From<$t> for Integer {
                #[inline]
                fn from(value: $t) -> Self {
                    Self(i128::from(value))
                }
            }

            impl TryFrom<Integer> for $t {
                type Error = core::num::TryFromIntError;

                #[inline]
                fn try_from(value: Integer) -> Result<Self, Self::Error> {
                    $t::try_from(value.0)
                }
            }
        )+
    };
}

/// An abstract integer value
///
/// This opaque type represents an integer value which can be encoded in CBOR
/// without resulting to big integer encoding. Larger values may be encoded
/// using the big integer encoding as described in the CBOR RFC. See the
/// implementations for 128-bit integer conversions on `Value` for more
/// details.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Integer(i128);

impl Integer {
    /// Returns the canonical length this integer will have when serialized to bytes.
    /// This is called `canonical` as it is only used for canonically comparing two
    /// values. It shouldn't be used in any other context.
    fn canonical_len(&self) -> usize {
        let x = self.0;

        if (0..=i128::from(u8::MAX)).contains(&x) {
            if x < 24 {
                1
            } else {
                2
            }
        } else if (i128::from(i8::MIN)..=i128::from(i8::MAX)).contains(&x) {
            if x >= -24 {
                1
            } else {
                2
            }
        } else if u16::try_from(x).is_ok() || i16::try_from(x).is_ok() {
            3
        } else if u32::try_from(x).is_ok() || i32::try_from(x).is_ok() {
            5
        } else if u64::try_from(x).is_ok() || i64::try_from(x).is_ok() {
            9
        } else {
            // Ciborium serializes u128/i128 as BigPos if they don't fit in 64 bits.
            // In this special case we have to calculate the length.
            // The Tag itself will always be 1 byte.
            x.to_be_bytes().len().saturating_add(1)
        }
    }

    /// Compare two integers as if we were to serialize them, but more efficiently.
    #[must_use]
    pub fn canonical_cmp(&self, other: &Self) -> Ordering {
        let length_order = self.canonical_len().cmp(&other.canonical_len());
        if length_order != Ordering::Equal {
            return length_order;
        }
        // Negative numbers are higher in byte-order than positive numbers.
        match (self.0.is_negative(), other.0.is_negative()) {
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            (true, true) => {
                // For negative numbers the byte order puts numbers closer to 0 which
                // are lexically higher, lower. So -1 < -2 when sorting by be_bytes().
                match self.0.cmp(&other.0) {
                    Ordering::Less => Ordering::Greater,
                    Ordering::Equal => Ordering::Equal,
                    Ordering::Greater => Ordering::Less,
                }
            }
            (_, _) => self.0.cmp(&other.0),
        }
    }
}

implfrom! {
    u8 u16 u32 u64
    i8 i16 i32 i64
}

// Pointer-sized integers have no standard From conversion into i128. Extend
// their bytes instead; every supported pointer value fits without truncation.
#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
impl From<usize> for Integer {
    #[inline]
    fn from(value: usize) -> Self {
        let input = value.to_le_bytes();
        let mut output = [0; 16];
        for (byte, input_byte) in output.iter_mut().zip(input) {
            *byte = input_byte;
        }
        Self(i128::from_le_bytes(output))
    }
}

#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
impl From<isize> for Integer {
    #[inline]
    fn from(value: isize) -> Self {
        let input = value.to_le_bytes();
        let mut output = [if value.is_negative() { 0xff } else { 0 }; 16];
        for (byte, input_byte) in output.iter_mut().zip(input) {
            *byte = input_byte;
        }
        Self(i128::from_le_bytes(output))
    }
}

#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
impl TryFrom<Integer> for usize {
    type Error = core::num::TryFromIntError;

    #[inline]
    fn try_from(value: Integer) -> Result<Self, Self::Error> {
        Self::try_from(value.0)
    }
}

#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
impl TryFrom<Integer> for isize {
    type Error = core::num::TryFromIntError;

    #[inline]
    fn try_from(value: Integer) -> Result<Self, Self::Error> {
        Self::try_from(value.0)
    }
}

impl TryFrom<i128> for Integer {
    type Error = core::num::TryFromIntError;

    #[inline]
    fn try_from(value: i128) -> Result<Self, Self::Error> {
        let _checked_magnitude = u64::try_from(if value.is_negative() {
            value ^ !0
        } else {
            value
        })?;

        Ok(Integer(value))
    }
}

impl TryFrom<u128> for Integer {
    type Error = core::num::TryFromIntError;

    #[inline]
    fn try_from(value: u128) -> Result<Self, Self::Error> {
        Ok(Self(u64::try_from(value)?.into()))
    }
}

impl From<Integer> for i128 {
    #[inline]
    fn from(value: Integer) -> Self {
        value.0
    }
}

impl TryFrom<Integer> for u128 {
    type Error = core::num::TryFromIntError;

    #[inline]
    fn try_from(value: Integer) -> Result<Self, Self::Error> {
        u128::try_from(value.0)
    }
}
