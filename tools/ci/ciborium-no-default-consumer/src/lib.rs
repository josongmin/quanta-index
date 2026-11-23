//! A standalone `no_std` consumer of registry Ciborium without default features.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use ciborium::{de::from_reader, ser::into_writer};

/// Exercises both registry codec directions in code compiled without `std`.
pub fn no_std_roundtrip() -> bool {
    let mut wire = Vec::new();
    into_writer(&7_u8, &mut wire).is_ok()
        && wire == [7]
        && matches!(from_reader::<u8, &[u8]>(wire.as_slice()), Ok(7))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_compiles_and_runs_codec_without_std() {
        assert!(no_std_roundtrip());
    }

    #[test]
    fn decodes_fixed_scalar_wire() {
        assert!(matches!(from_reader::<u8, &[u8]>(&[7][..]), Ok(7)));
    }

    #[test]
    fn rejects_empty_input() {
        assert!(from_reader::<u8, &[u8]>(&[]).is_err());
    }

    #[test]
    fn encodes_into_fixed_slice() {
        let mut wire = [0; 1];
        assert!(into_writer(&3_u8, &mut wire[..]).is_ok());
        assert_eq!(wire, [3]);
    }

    #[test]
    fn encodes_into_alloc_vec() {
        let mut wire = Vec::new();
        assert!(into_writer(&3_u8, &mut wire).is_ok());
        assert_eq!(wire, [3]);
    }

    #[test]
    fn rejects_exhausted_writer() {
        assert!(into_writer(&3_u8, &mut [][..]).is_err());
    }
}
