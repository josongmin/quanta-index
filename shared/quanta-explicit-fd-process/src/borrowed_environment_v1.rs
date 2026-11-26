//! Checked view over caller-owned environment storage. No entry or pointer
//! backing is allocated here; the caller's native custody covers both slices.

use crate::ExplicitFdProcessErrorV1;
use std::ffi::{c_char, CString};

#[derive(Clone, Copy, Debug)]
pub struct BorrowedEnvironmentV1<'env> {
    _entries_v1: &'env [CString],
    #[cfg(target_os = "linux")]
    pointers_v1: &'env [*mut c_char],
}

impl<'env> BorrowedEnvironmentV1<'env> {
    pub fn try_new_v1(
        entries_v1: &'env [CString],
        pointers_v1: &'env [*mut c_char],
    ) -> Result<Self, ExplicitFdProcessErrorV1> {
        if entries_v1.len().checked_add(1) != Some(pointers_v1.len())
            || !pointers_v1
                .last()
                .is_some_and(|pointer_v1| pointer_v1.is_null())
            || entries_v1
                .iter()
                .zip(pointers_v1)
                .any(|(entry_v1, pointer_v1)| entry_v1.as_ptr().cast_mut() != *pointer_v1)
        {
            return Err(ExplicitFdProcessErrorV1::InvalidBorrowedEnvironment);
        }
        Ok(Self {
            _entries_v1: entries_v1,
            #[cfg(target_os = "linux")]
            pointers_v1,
        })
    }

    pub fn empty_v1() -> Self {
        Self {
            _entries_v1: &[],
            #[cfg(target_os = "linux")]
            pointers_v1: &[std::ptr::null_mut()],
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn pointers_v1(self) -> &'env [*mut c_char] {
        self.pointers_v1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_view_requires_exact_live_entry_pointers_and_final_null() {
        let entries_v1 = [CString::new("A=1").unwrap(), CString::new("B=2").unwrap()];
        let pointers_v1 = [
            entries_v1[0].as_ptr().cast_mut(),
            entries_v1[1].as_ptr().cast_mut(),
            std::ptr::null_mut(),
        ];
        let _view_v1 = BorrowedEnvironmentV1::try_new_v1(&entries_v1, &pointers_v1).unwrap();
        #[cfg(target_os = "linux")]
        assert!(std::ptr::eq(
            _view_v1.pointers_v1().as_ptr(),
            pointers_v1.as_ptr()
        ));
        #[cfg(target_os = "linux")]
        assert_eq!(BorrowedEnvironmentV1::empty_v1().pointers_v1().len(), 1);
        for invalid_v1 in [
            &pointers_v1[..2],
            &[
                entries_v1[1].as_ptr().cast_mut(),
                entries_v1[0].as_ptr().cast_mut(),
                std::ptr::null_mut(),
            ][..],
            &[
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ][..],
        ] {
            assert!(matches!(
                BorrowedEnvironmentV1::try_new_v1(&entries_v1, invalid_v1),
                Err(ExplicitFdProcessErrorV1::InvalidBorrowedEnvironment)
            ));
        }
    }
}
