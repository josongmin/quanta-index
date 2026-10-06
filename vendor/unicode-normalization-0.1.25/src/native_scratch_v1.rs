//! Admission policy for the original canonical normalization iterator.

use crate::recompose::Recompositions;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use core::{
    convert::{Infallible, TryFrom},
    fmt,
    mem::size_of,
};
#[cfg(feature = "std")]
use std::vec::Vec;
use tinyvec::{Array, TinyVec};

/// Actual scratch owner within one synchronous normalization call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeNormalizationScratchOwnerV1 {
    /// Canonical decomposition and ordering pairs.
    Decomposition,
    /// Canonical recomposition's pending characters.
    Recomposition,
    /// Stable ordering's temporary pairs in the unbounded streaming rail.
    Sort,
}

/// Heap backing present immediately before and after a native growth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeNormalizationScratchDemandV1 {
    /// The buffer whose lifetime and growth are being admitted.
    pub owner_v1: NativeNormalizationScratchOwnerV1,
    /// Existing heap bytes; inline storage contributes zero.
    pub current_bytes_v1: usize,
    /// Replacement heap bytes, simultaneously admitted with existing bytes.
    pub new_bytes_v1: usize,
}

/// Borrowed, synchronous admission of canonical normalization work and backing.
///
/// The policy must retain each buffer's backing admission until release. During
/// growth it retains old and replacement backing across `birth`, then may
/// release the old backing. No callback, policy or grant escapes the call.
pub trait NativeNormalizationAdmissionV1 {
    /// The original caller's exact refusal type.
    type Error;

    /// Consume before an iterator step, decomposition, emitted scalar, buffer
    /// move or comparison. Ordering consumes one unit per pending scalar
    /// before the original stable sort (not per implementation comparison).
    fn checkpoint_work_v1(&mut self, units_v1: u64) -> Result<(), Self::Error>;

    /// Admit the actual growth before invoking `birth` exactly once. Return
    /// its native success unchanged; admission refusal must not invoke it.
    fn native_birth_v1(
        &mut self,
        demand_v1: NativeNormalizationScratchDemandV1,
        birth_v1: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error>;

    /// Release custody after both actual iterator buffers have been dropped.
    /// Called on success and every failure, even for an inline-only buffer.
    fn release_scratch_v1(&mut self, owner_v1: NativeNormalizationScratchOwnerV1);
}

/// A typed refusal from the original producer; none is an NFC verdict.
#[derive(Debug, PartialEq, Eq)]
pub enum NativeNormalizationErrorV1<E> {
    /// Original admission failure without a diagnostic allocation.
    Admission(E),
    /// The admitted fallible native allocation failed.
    NativeAllocationFailed,
    /// The caller did not invoke exactly one native birth or changed its result.
    InvalidNativeProducer,
    /// Native capacity did not match the before-birth plan.
    InvalidNativeCapacity,
    /// A demand cannot be represented in the native or work counter domain.
    ArithmeticOverflow,
    /// The sort shape lies outside the pinned, stack-only standard-library rail.
    UnsupportedNativeSortBacking,
    /// This controlled producer is for the canonical 512-byte identity rail.
    InputTooLong,
}

impl<E: fmt::Display> fmt::Display for NativeNormalizationErrorV1<E> {
    fn fmt(&self, formatter_v1: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(cause_v1) => {
                write!(formatter_v1, "normalization admission: {cause_v1}")
            }
            Self::NativeAllocationFailed => {
                formatter_v1.write_str("normalization native allocation failed")
            }
            Self::InvalidNativeProducer => {
                formatter_v1.write_str("normalization native producer is invalid")
            }
            Self::InvalidNativeCapacity => {
                formatter_v1.write_str("normalization native capacity is invalid")
            }
            Self::ArithmeticOverflow => {
                formatter_v1.write_str("normalization demand arithmetic overflow")
            }
            Self::UnsupportedNativeSortBacking => {
                formatter_v1.write_str("normalization sort backing is unsupported")
            }
            Self::InputTooLong => {
                formatter_v1.write_str("normalization input exceeds 512 UTF8 bytes")
            }
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for NativeNormalizationErrorV1<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(cause_v1) => Some(cause_v1),
            _ => None,
        }
    }
}

pub(crate) trait NormalizationPolicyV1 {
    type Error;
    fn work_v1(&mut self, units_v1: usize) -> Result<(), Self::Error>;
    fn push_decomposition_v1(
        &mut self,
        buffer_v1: &mut TinyVec<[(u8, char); 4]>,
        value_v1: (u8, char),
    ) -> Result<(), Self::Error>;
    fn push_recomposition_v1(
        &mut self,
        buffer_v1: &mut TinyVec<[char; 4]>,
        value_v1: char,
    ) -> Result<(), Self::Error>;
    fn sort_v1(&mut self, pending_v1: &mut [(u8, char)]) -> Result<(), Self::Error>;
}

pub(crate) struct OrdinaryNormalizationPolicyV1;

impl NormalizationPolicyV1 for OrdinaryNormalizationPolicyV1 {
    type Error = Infallible;
    fn work_v1(&mut self, _: usize) -> Result<(), Infallible> {
        Ok(())
    }
    fn push_decomposition_v1(
        &mut self,
        buffer_v1: &mut TinyVec<[(u8, char); 4]>,
        value_v1: (u8, char),
    ) -> Result<(), Infallible> {
        buffer_v1.push(value_v1);
        Ok(())
    }
    fn push_recomposition_v1(
        &mut self,
        buffer_v1: &mut TinyVec<[char; 4]>,
        value_v1: char,
    ) -> Result<(), Infallible> {
        buffer_v1.push(value_v1);
        Ok(())
    }
    fn sort_v1(&mut self, pending_v1: &mut [(u8, char)]) -> Result<(), Infallible> {
        pending_v1.sort_by_key(|value_v1| value_v1.0);
        Ok(())
    }
}

struct ControlledNormalizationPolicyV1<'a, P> {
    admission_v1: &'a mut P,
    streaming_v1: bool,
}

impl<P: NativeNormalizationAdmissionV1> ControlledNormalizationPolicyV1<'_, P> {
    fn push_v1<A: Array>(
        &mut self,
        owner_v1: NativeNormalizationScratchOwnerV1,
        buffer_v1: &mut TinyVec<A>,
        value_v1: A::Item,
    ) -> Result<(), NativeNormalizationErrorV1<P::Error>> {
        use NativeNormalizationErrorV1 as E;
        if buffer_v1.len() == buffer_v1.capacity() {
            // Match the original TinyVec inline spill and Vec growth: double
            // full capacity. Exact reserve makes the actual backing auditable.
            let new_capacity_v1 = buffer_v1
                .capacity()
                .checked_mul(2)
                .ok_or(E::ArithmeticOverflow)?;
            let current_bytes_v1 = if buffer_v1.is_heap() {
                buffer_v1
                    .capacity()
                    .checked_mul(size_of::<A::Item>())
                    .ok_or(E::ArithmeticOverflow)?
            } else {
                0
            };
            let new_bytes_v1 = new_capacity_v1
                .checked_mul(size_of::<A::Item>())
                .ok_or(E::ArithmeticOverflow)?;
            let additional_v1 = new_capacity_v1
                .checked_sub(buffer_v1.len())
                .ok_or(E::ArithmeticOverflow)?;
            let mut invoked_v1 = false;
            let mut repeated_v1 = false;
            let mut native_success_v1 = false;
            let admitted_v1 = self
                .admission_v1
                .native_birth_v1(
                    NativeNormalizationScratchDemandV1 {
                        owner_v1,
                        current_bytes_v1,
                        new_bytes_v1,
                    },
                    &mut || {
                        if invoked_v1 {
                            repeated_v1 = true;
                            return false;
                        }
                        invoked_v1 = true;
                        native_success_v1 = buffer_v1.try_reserve_exact(additional_v1).is_ok();
                        native_success_v1
                    },
                )
                .map_err(E::Admission)?;
            if !invoked_v1 || repeated_v1 || admitted_v1 != native_success_v1 {
                return Err(E::InvalidNativeProducer);
            }
            if !native_success_v1 {
                return Err(E::NativeAllocationFailed);
            }
            if buffer_v1.capacity() != new_capacity_v1 {
                return Err(E::InvalidNativeCapacity);
            }
        }
        buffer_v1.push(value_v1);
        Ok(())
    }
}

impl<P: NativeNormalizationAdmissionV1> NormalizationPolicyV1
    for ControlledNormalizationPolicyV1<'_, P>
{
    type Error = NativeNormalizationErrorV1<P::Error>;
    fn work_v1(&mut self, units_v1: usize) -> Result<(), Self::Error> {
        let units_v1 =
            u64::try_from(units_v1).map_err(|_| NativeNormalizationErrorV1::ArithmeticOverflow)?;
        self.admission_v1
            .checkpoint_work_v1(units_v1)
            .map_err(NativeNormalizationErrorV1::Admission)
    }
    fn push_decomposition_v1(
        &mut self,
        buffer_v1: &mut TinyVec<[(u8, char); 4]>,
        value_v1: (u8, char),
    ) -> Result<(), Self::Error> {
        self.push_v1(
            NativeNormalizationScratchOwnerV1::Decomposition,
            buffer_v1,
            value_v1,
        )
    }
    fn push_recomposition_v1(
        &mut self,
        buffer_v1: &mut TinyVec<[char; 4]>,
        value_v1: char,
    ) -> Result<(), Self::Error> {
        self.push_v1(
            NativeNormalizationScratchOwnerV1::Recomposition,
            buffer_v1,
            value_v1,
        )
    }
    fn sort_v1(&mut self, pending_v1: &mut [(u8, char)]) -> Result<(), Self::Error> {
        if self.streaming_v1 && pending_v1.len() > 512 {
            // Counting by canonical combining class preserves the original
            // stable order while making the only large sort backing explicit.
            // The 256 counters live on the stack; pairs live in admitted Vec.
            let bytes_v1 = pending_v1
                .len()
                .checked_mul(size_of::<(u8, char)>())
                .ok_or(NativeNormalizationErrorV1::ArithmeticOverflow)?;
            let work_v1 = pending_v1
                .len()
                .checked_mul(3)
                .ok_or(NativeNormalizationErrorV1::ArithmeticOverflow)?;
            self.work_v1(work_v1)?;
            let result_v1 = (|| {
                let mut ordered_v1 = Vec::new();
                let mut invoked_v1 = false;
                let mut repeated_v1 = false;
                let mut native_success_v1 = false;
                let admitted_v1 = self
                    .admission_v1
                    .native_birth_v1(
                        NativeNormalizationScratchDemandV1 {
                            owner_v1: NativeNormalizationScratchOwnerV1::Sort,
                            current_bytes_v1: 0,
                            new_bytes_v1: bytes_v1,
                        },
                        &mut || {
                            if invoked_v1 {
                                repeated_v1 = true;
                                return false;
                            }
                            invoked_v1 = true;
                            native_success_v1 =
                                ordered_v1.try_reserve_exact(pending_v1.len()).is_ok();
                            native_success_v1
                        },
                    )
                    .map_err(NativeNormalizationErrorV1::Admission)?;
                if !invoked_v1 || repeated_v1 || admitted_v1 != native_success_v1 {
                    return Err(NativeNormalizationErrorV1::InvalidNativeProducer);
                }
                if !native_success_v1 {
                    return Err(NativeNormalizationErrorV1::NativeAllocationFailed);
                }
                if ordered_v1.capacity() != pending_v1.len() {
                    return Err(NativeNormalizationErrorV1::InvalidNativeCapacity);
                }
                ordered_v1.resize(pending_v1.len(), (0, '\0'));
                let mut positions_v1 = [0_usize; 256];
                for &(class_v1, _) in pending_v1.iter() {
                    positions_v1[usize::from(class_v1)] += 1;
                }
                let mut next_v1 = 0;
                for position_v1 in &mut positions_v1 {
                    let count_v1 = *position_v1;
                    *position_v1 = next_v1;
                    next_v1 += count_v1;
                }
                for &pair_v1 in pending_v1.iter() {
                    let position_v1 = &mut positions_v1[usize::from(pair_v1.0)];
                    ordered_v1[*position_v1] = pair_v1;
                    *position_v1 += 1;
                }
                pending_v1.copy_from_slice(&ordered_v1);
                Ok(())
            })();
            self.admission_v1
                .release_scratch_v1(NativeNormalizationScratchOwnerV1::Sort);
            return result_v1;
        }
        // Pinned Rust 1.92 driftsort uses 4096-byte stack scratch for <=512
        // eight-byte pairs. Unicode 17 canonical pending suffixes consume no
        // more scalars than their input UTF8 bytes. No native sort birth occurs
        // on this <=512-byte identity rail; the ordinary sort is unchanged.
        if !cfg!(all(
            target_pointer_width = "64",
            any(target_os = "linux", target_os = "macos")
        )) || size_of::<(u8, char)>() != 8
            || pending_v1.len() > 512
        {
            return Err(NativeNormalizationErrorV1::UnsupportedNativeSortBacking);
        }
        pending_v1.sort_by_key(|value_v1| value_v1.0);
        Ok(())
    }
}

/// Compare the original canonical NFC iterator with the borrowed identity.
///
/// This never normalizes or creates an owned output. The original canonical
/// decomposition, stable ordering and recomposition state machines are shared
/// with ordinary `.nfc()`. The controlled rail is pinned Rust 1.92 on 64-bit
/// Linux/macOS and input of at most 512 UTF8 bytes. Policies borrow original
/// control; arbitrary caller-supplied NFC verdicts cannot bypass validation.
#[cfg(feature = "quanta-native-scratch-v1")]
pub fn try_is_nfc_with_native_admission_v1<P: NativeNormalizationAdmissionV1>(
    input_v1: &str,
    admission_v1: &mut P,
) -> Result<bool, NativeNormalizationErrorV1<P::Error>> {
    let result_v1 = (|| {
        let mut policy_v1 = ControlledNormalizationPolicyV1 {
            admission_v1: &mut *admission_v1,
            streaming_v1: false,
        };
        policy_v1.work_v1(0)?;
        if input_v1.len() > 512 {
            return Err(NativeNormalizationErrorV1::InputTooLong);
        }
        let mut normalized_v1 = Recompositions::new_canonical(input_v1.chars());
        let mut original_v1 = input_v1.chars();
        loop {
            let left_v1 = normalized_v1.try_next_with_policy_v1(&mut policy_v1)?;
            policy_v1.work_v1(1)?;
            let right_v1 = original_v1.next();
            match (left_v1, right_v1) {
                (None, None) => return Ok(true),
                (Some(left_v1), Some(right_v1)) if left_v1 == right_v1 => {}
                _ => return Ok(false),
            }
        }
    })();
    // All normalization buffers have dropped before custody is released.
    admission_v1.release_scratch_v1(NativeNormalizationScratchOwnerV1::Decomposition);
    admission_v1.release_scratch_v1(NativeNormalizationScratchOwnerV1::Recomposition);
    result_v1
}

/// Stream canonical NFC scalars through the same decomposition and
/// recomposition machines as `.nfc()`, admitting native scratch before birth.
/// Unlike the borrowed identity rail, this accepts caller-bounded long text.
#[cfg(feature = "quanta-native-scratch-v1")]
pub fn try_for_each_nfc_with_native_admission_v1<P, F>(
    input_v1: &str,
    admission_v1: &mut P,
    mut emit_v1: F,
) -> Result<(), NativeNormalizationErrorV1<P::Error>>
where
    P: NativeNormalizationAdmissionV1,
    F: FnMut(char) -> Result<(), P::Error>,
{
    let result_v1 = (|| {
        let mut policy_v1 = ControlledNormalizationPolicyV1 {
            admission_v1: &mut *admission_v1,
            streaming_v1: true,
        };
        policy_v1.work_v1(0)?;
        let mut normalized_v1 = Recompositions::new_canonical(input_v1.chars());
        while let Some(scalar_v1) = normalized_v1.try_next_with_policy_v1(&mut policy_v1)? {
            emit_v1(scalar_v1).map_err(NativeNormalizationErrorV1::Admission)?;
        }
        Ok(())
    })();
    admission_v1.release_scratch_v1(NativeNormalizationScratchOwnerV1::Decomposition);
    admission_v1.release_scratch_v1(NativeNormalizationScratchOwnerV1::Recomposition);
    result_v1
}

#[cfg(test)]
mod sort_backing_bound_tests_v1 {
    #[test]
    fn canonical_table_nonstarter_runs_fit_original_utf8_bytes_v1() {
        for &(scalar_v1, (start_v1, length_v1)) in crate::tables::CANONICAL_DECOMPOSED_KV {
            if length_v1 == 0 {
                continue;
            }
            let original_v1 = char::from_u32(scalar_v1).unwrap();
            let mut run_v1 = 0;
            for &decomposed_v1 in &crate::tables::CANONICAL_DECOMPOSED_CHARS
                [usize::from(start_v1)..][..usize::from(length_v1)]
            {
                if crate::char::canonical_combining_class(decomposed_v1) == 0 {
                    run_v1 = 0;
                } else {
                    run_v1 += 1;
                    assert!(run_v1 <= original_v1.len_utf8());
                }
            }
        }
        // Algorithmic Hangul decompositions are not in the canonical table.
        for scalar_v1 in 0xAC00..0xD7A4 {
            crate::char::decompose_canonical(char::from_u32(scalar_v1).unwrap(), |value_v1| {
                assert_eq!(crate::char::canonical_combining_class(value_v1), 0);
            });
        }
    }
}
