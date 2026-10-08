//! Admission policy for the original canonical normalization iterator.

#[cfg(feature = "quanta-native-scratch-v1")]
use crate::recompose::Recompositions;
#[cfg(all(feature = "quanta-native-scratch-v1", not(feature = "std")))]
use alloc::vec::Vec;
use core::convert::Infallible;
#[cfg(feature = "quanta-native-scratch-v1")]
use core::{convert::TryFrom, fmt, mem::size_of};
#[cfg(all(feature = "quanta-native-scratch-v1", feature = "std"))]
use std::vec::Vec;
#[cfg(feature = "quanta-native-scratch-v1")]
use tinyvec::Array;
use tinyvec::TinyVec;

/// Actual scratch owner within one synchronous normalization call.
#[cfg(feature = "quanta-native-scratch-v1")]
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
#[cfg(feature = "quanta-native-scratch-v1")]
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
/// release the old backing. No callback or policy escapes the producer. An
/// into-DATA caller keeps the actual funding bank outside that transient policy.
#[cfg(feature = "quanta-native-scratch-v1")]
pub trait NativeNormalizationAdmissionV1 {
    /// The original caller's exact refusal type.
    type Error;

    /// Consume before an iterator step, decomposition, emitted scalar, buffer
    /// move or comparison. Ordering prepays aggregate work for pending scalars;
    /// large streaming runs also prepay their counting-sort passes. These are
    /// work units, not one callback per implementation comparison.
    fn checkpoint_work_v1(&mut self, units_v1: u64) -> Result<(), Self::Error>;

    /// Admit the actual growth before invoking `birth` exactly once. Return
    /// its native success unchanged. Refusal before birth must not invoke it.
    /// A later error must retain admission for all backing actually born until
    /// the caller retires DATA, even though no successful status is returned.
    fn native_birth_v1(
        &mut self,
        demand_v1: NativeNormalizationScratchDemandV1,
        birth_v1: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error>;

    /// Release custody after the backing for this owner has been dropped.
    /// Owned conveniences release at call end. Into-DATA producers do not
    /// release: the caller retains its actual funding bank with the DATA
    /// through the highest source finisher, then drops backing before grants.
    fn release_scratch_v1(&mut self, owner_v1: NativeNormalizationScratchOwnerV1);
}

/// A typed refusal from the original producer; none is an NFC verdict.
#[cfg(feature = "quanta-native-scratch-v1")]
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

#[cfg(feature = "quanta-native-scratch-v1")]
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

#[cfg(all(feature = "quanta-native-scratch-v1", feature = "std"))]
impl<E: std::error::Error + 'static> std::error::Error for NativeNormalizationErrorV1<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(cause_v1) => Some(cause_v1),
            _ => None,
        }
    }
}

/// A canonical normalization result, without source or lifecycle authority.
#[cfg(feature = "quanta-native-scratch-v1")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeNormalizationOutcomeV1 {
    /// Whether the borrowed input was already canonical NFC.
    IsNfc(bool),
    /// Every normalized scalar was delivered to the caller's emitter.
    Streamed,
}

/// Status-only refusal. The full normalization error stays in external DATA;
/// this value never replaces that cause or carries source authority.
#[cfg(feature = "quanta-native-scratch-v1")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeNormalizationDataRefusalV1 {
    /// This one-attempt DATA was already used or retired.
    UsedData,
    /// The operation's full error has been recorded in DATA.
    OperationRefused,
    /// A pure transfer must preserve the occupied caller output.
    OccupiedOutput,
    /// There is no recorded result to transfer.
    MissingResult,
}

/// One source-free external attempt. No input borrow or admission escapes.
///
/// Actual iterator and sort backing survive success and all finite refusals,
/// including admission refusal after native birth. Full errors stay here until
/// a pure transfer to another caller-owned result slot. Reuse never polls or
/// overwrites the first result. This DATA grants no source authority.
///
/// The caller must retain the actual funding bank alongside this DATA through
/// its highest source finisher. Drop this DATA before that bank; storing only
/// accounting bytes cannot keep the original grants alive.
#[cfg(feature = "quanta-native-scratch-v1")]
pub struct NativeNormalizationDataV1<E> {
    // Drop real backing before an error that may itself retain original grants.
    decomposition_v1: TinyVec<[(u8, char); 4]>,
    recomposition_v1: TinyVec<[char; 4]>,
    sort_v1: Vec<(u8, char)>,
    sort_attempted_v1: bool,
    attempted_v1: bool,
    released_v1: bool,
    result_v1: Option<Result<NativeNormalizationOutcomeV1, NativeNormalizationErrorV1<E>>>,
}

#[cfg(feature = "quanta-native-scratch-v1")]
impl<E> NativeNormalizationDataV1<E> {
    /// Create empty, allocation-free storage for one normalization attempt.
    pub fn new_v1() -> Self {
        Self {
            decomposition_v1: TinyVec::new(),
            recomposition_v1: TinyVec::new(),
            sort_v1: Vec::new(),
            sort_attempted_v1: false,
            attempted_v1: false,
            released_v1: false,
            result_v1: None,
        }
    }

    /// Whether this DATA has never been attempted or retired.
    pub fn is_fresh_v1(&self) -> bool {
        !self.attempted_v1 && !self.released_v1
    }

    /// Observe the complete result without moving or cloning its error.
    pub fn result_v1(
        &self,
    ) -> Option<Result<NativeNormalizationOutcomeV1, &NativeNormalizationErrorV1<E>>> {
        self.result_v1.as_ref().map(|result_v1| match result_v1 {
            Ok(outcome_v1) => Ok(*outcome_v1),
            Err(cause_v1) => Err(cause_v1),
        })
    }

    /// Pure transfer after the finisher, or between external DATA before it.
    /// Occupied outputs and missing results are preserved without admission.
    pub fn result_into_slot_v1(
        &mut self,
        output_v1: &mut Option<Result<NativeNormalizationOutcomeV1, NativeNormalizationErrorV1<E>>>,
    ) -> Result<(), NativeNormalizationDataRefusalV1> {
        if output_v1.is_some() {
            return Err(NativeNormalizationDataRefusalV1::OccupiedOutput);
        }
        if self.result_v1.is_none() {
            return Err(NativeNormalizationDataRefusalV1::MissingResult);
        }
        *output_v1 = self.result_v1.take();
        Ok(())
    }

    /// Retire only at the caller's safe terminal point. All actual backing is
    /// dropped before any release callback; the full result remains in DATA.
    /// Protected callers may instead drop DATA then their external funding bank
    /// without keeping a transient admission alive after the source finisher.
    pub fn release_scratch_v1<P: NativeNormalizationAdmissionV1<Error = E>>(
        &mut self,
        admission_v1: &mut P,
    ) {
        if !self.attempted_v1 || self.released_v1 {
            return;
        }
        self.released_v1 = true;
        drop(core::mem::take(&mut self.decomposition_v1));
        drop(core::mem::take(&mut self.recomposition_v1));
        drop(core::mem::take(&mut self.sort_v1));
        if self.sort_attempted_v1 {
            admission_v1.release_scratch_v1(NativeNormalizationScratchOwnerV1::Sort);
        }
        admission_v1.release_scratch_v1(NativeNormalizationScratchOwnerV1::Decomposition);
        admission_v1.release_scratch_v1(NativeNormalizationScratchOwnerV1::Recomposition);
    }

    fn record_v1(
        &mut self,
        result_v1: Result<NativeNormalizationOutcomeV1, NativeNormalizationErrorV1<E>>,
    ) -> Result<(), NativeNormalizationDataRefusalV1> {
        let success_v1 = result_v1.is_ok();
        self.result_v1 = Some(result_v1);
        if success_v1 {
            Ok(())
        } else {
            Err(NativeNormalizationDataRefusalV1::OperationRefused)
        }
    }
}

#[cfg(feature = "quanta-native-scratch-v1")]
impl<E> Default for NativeNormalizationDataV1<E> {
    fn default() -> Self {
        Self::new_v1()
    }
}

#[cfg(feature = "quanta-native-scratch-v1")]
impl<E> fmt::Debug for NativeNormalizationDataV1<E> {
    fn fmt(&self, formatter_v1: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter_v1
            .debug_struct("NativeNormalizationDataV1")
            .field("attempted", &self.attempted_v1)
            .field("has_result", &self.result_v1.is_some())
            .finish_non_exhaustive()
    }
}

// The canonical iterator remains the sole algorithm. This transient runner
// returns its buffers even on unwind; it never puts input borrows in DATA.
#[cfg(feature = "quanta-native-scratch-v1")]
struct NormalizationRunnerV1<'data, 'input> {
    normalized_v1: Recompositions<core::str::Chars<'input>>,
    decomposition_v1: &'data mut TinyVec<[(u8, char); 4]>,
    recomposition_v1: &'data mut TinyVec<[char; 4]>,
}

#[cfg(feature = "quanta-native-scratch-v1")]
impl<'data, 'input> NormalizationRunnerV1<'data, 'input> {
    fn new_v1(
        input_v1: &'input str,
        decomposition_v1: &'data mut TinyVec<[(u8, char); 4]>,
        recomposition_v1: &'data mut TinyVec<[char; 4]>,
    ) -> Self {
        let mut normalized_v1 = Recompositions::new_canonical(input_v1.chars());
        normalized_v1.swap_scratch_v1(decomposition_v1, recomposition_v1);
        Self {
            normalized_v1,
            decomposition_v1,
            recomposition_v1,
        }
    }
}

#[cfg(feature = "quanta-native-scratch-v1")]
impl Drop for NormalizationRunnerV1<'_, '_> {
    fn drop(&mut self) {
        self.normalized_v1
            .swap_scratch_v1(self.decomposition_v1, self.recomposition_v1);
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

#[cfg(feature = "quanta-native-scratch-v1")]
struct ControlledNormalizationPolicyV1<'a, P> {
    admission_v1: &'a mut P,
    streaming_v1: bool,
    sort_backing_v1: &'a mut Vec<(u8, char)>,
    sort_attempted_v1: &'a mut bool,
}

#[cfg(feature = "quanta-native-scratch-v1")]
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

#[cfg(feature = "quanta-native-scratch-v1")]
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
            if self.sort_backing_v1.capacity() < pending_v1.len() {
                let current_bytes_v1 = self
                    .sort_backing_v1
                    .capacity()
                    .checked_mul(size_of::<(u8, char)>())
                    .ok_or(NativeNormalizationErrorV1::ArithmeticOverflow)?;
                self.sort_backing_v1.clear();
                let mut invoked_v1 = false;
                let mut repeated_v1 = false;
                let mut native_success_v1 = false;
                *self.sort_attempted_v1 = true;
                let sort_backing_v1 = &mut *self.sort_backing_v1;
                let admitted_v1 = self
                    .admission_v1
                    .native_birth_v1(
                        NativeNormalizationScratchDemandV1 {
                            owner_v1: NativeNormalizationScratchOwnerV1::Sort,
                            current_bytes_v1,
                            new_bytes_v1: bytes_v1,
                        },
                        &mut || {
                            if invoked_v1 {
                                repeated_v1 = true;
                                return false;
                            }
                            invoked_v1 = true;
                            native_success_v1 =
                                sort_backing_v1.try_reserve_exact(pending_v1.len()).is_ok();
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
                if self.sort_backing_v1.capacity() != pending_v1.len() {
                    return Err(NativeNormalizationErrorV1::InvalidNativeCapacity);
                }
            }
            self.sort_backing_v1.resize(pending_v1.len(), (0, '\0'));
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
                self.sort_backing_v1[*position_v1] = pair_v1;
                *position_v1 += 1;
            }
            pending_v1.copy_from_slice(self.sort_backing_v1);
            return Ok(());
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
///
/// Returns only attempt status. The full outcome or error is stored in DATA
/// before return, with every actual scratch backing. No grant is released.
/// Used DATA is rejected before input/admission polling and remains unchanged.
#[cfg(feature = "quanta-native-scratch-v1")]
pub fn try_is_nfc_into_with_native_admission_v1<P: NativeNormalizationAdmissionV1>(
    input_v1: &str,
    data_v1: &mut NativeNormalizationDataV1<P::Error>,
    admission_v1: &mut P,
) -> Result<(), NativeNormalizationDataRefusalV1> {
    if !data_v1.is_fresh_v1() {
        return Err(NativeNormalizationDataRefusalV1::UsedData);
    }
    data_v1.attempted_v1 = true;
    let result_v1 = (|| {
        let mut policy_v1 = ControlledNormalizationPolicyV1 {
            admission_v1: &mut *admission_v1,
            streaming_v1: false,
            sort_backing_v1: &mut data_v1.sort_v1,
            sort_attempted_v1: &mut data_v1.sort_attempted_v1,
        };
        policy_v1.work_v1(0)?;
        if input_v1.len() > 512 {
            return Err(NativeNormalizationErrorV1::InputTooLong);
        }
        let mut runner_v1 = NormalizationRunnerV1::new_v1(
            input_v1,
            &mut data_v1.decomposition_v1,
            &mut data_v1.recomposition_v1,
        );
        let mut original_v1 = input_v1.chars();
        loop {
            let left_v1 = runner_v1
                .normalized_v1
                .try_next_with_policy_v1(&mut policy_v1)?;
            policy_v1.work_v1(1)?;
            let right_v1 = original_v1.next();
            match (left_v1, right_v1) {
                (None, None) => return Ok(NativeNormalizationOutcomeV1::IsNfc(true)),
                (Some(left_v1), Some(right_v1)) if left_v1 == right_v1 => {}
                _ => return Ok(NativeNormalizationOutcomeV1::IsNfc(false)),
            }
        }
    })();
    data_v1.record_v1(result_v1)
}

/// Stream canonical NFC scalars through the same decomposition and
/// recomposition machines as `.nfc()`, admitting native scratch before birth.
/// Unlike the borrowed identity rail, this accepts caller-bounded long text.
/// Full normalization and emitter failures, plus actual scratch, remain in DATA.
/// Returns only status and never releases grants; used DATA is preserved
/// without polling admission, input, or the emitter.
#[cfg(feature = "quanta-native-scratch-v1")]
pub fn try_for_each_nfc_into_with_native_admission_v1<P, F>(
    input_v1: &str,
    data_v1: &mut NativeNormalizationDataV1<P::Error>,
    admission_v1: &mut P,
    mut emit_v1: F,
) -> Result<(), NativeNormalizationDataRefusalV1>
where
    P: NativeNormalizationAdmissionV1,
    F: FnMut(char) -> Result<(), P::Error>,
{
    if !data_v1.is_fresh_v1() {
        return Err(NativeNormalizationDataRefusalV1::UsedData);
    }
    data_v1.attempted_v1 = true;
    let result_v1 = (|| {
        let mut policy_v1 = ControlledNormalizationPolicyV1 {
            admission_v1: &mut *admission_v1,
            streaming_v1: true,
            sort_backing_v1: &mut data_v1.sort_v1,
            sort_attempted_v1: &mut data_v1.sort_attempted_v1,
        };
        policy_v1.work_v1(0)?;
        let mut runner_v1 = NormalizationRunnerV1::new_v1(
            input_v1,
            &mut data_v1.decomposition_v1,
            &mut data_v1.recomposition_v1,
        );
        while let Some(scalar_v1) = runner_v1
            .normalized_v1
            .try_next_with_policy_v1(&mut policy_v1)?
        {
            emit_v1(scalar_v1).map_err(NativeNormalizationErrorV1::Admission)?;
        }
        Ok(NativeNormalizationOutcomeV1::Streamed)
    })();
    data_v1.record_v1(result_v1)
}

#[cfg(feature = "quanta-native-scratch-v1")]
fn finish_local_data_v1<P: NativeNormalizationAdmissionV1>(
    mut data_v1: NativeNormalizationDataV1<P::Error>,
    admission_v1: &mut P,
) -> Result<NativeNormalizationOutcomeV1, NativeNormalizationErrorV1<P::Error>> {
    let mut result_v1 = None;
    let transferred_v1 = data_v1.result_into_slot_v1(&mut result_v1);
    data_v1.release_scratch_v1(admission_v1);
    if transferred_v1.is_err() {
        return Err(NativeNormalizationErrorV1::InvalidNativeProducer);
    }
    result_v1.ok_or(NativeNormalizationErrorV1::InvalidNativeProducer)?
}

/// Owned convenience over the same external-DATA producer. Use the into API
/// when scratch and full failures must survive a higher source finisher.
#[cfg(feature = "quanta-native-scratch-v1")]
pub fn try_is_nfc_with_native_admission_v1<P: NativeNormalizationAdmissionV1>(
    input_v1: &str,
    admission_v1: &mut P,
) -> Result<bool, NativeNormalizationErrorV1<P::Error>> {
    let mut data_v1 = NativeNormalizationDataV1::new_v1();
    let _status_v1 = try_is_nfc_into_with_native_admission_v1(input_v1, &mut data_v1, admission_v1);
    match finish_local_data_v1(data_v1, admission_v1)? {
        NativeNormalizationOutcomeV1::IsNfc(value_v1) => Ok(value_v1),
        NativeNormalizationOutcomeV1::Streamed => {
            Err(NativeNormalizationErrorV1::InvalidNativeProducer)
        }
    }
}

/// Owned streaming convenience over the same external-DATA producer.
#[cfg(feature = "quanta-native-scratch-v1")]
pub fn try_for_each_nfc_with_native_admission_v1<P, F>(
    input_v1: &str,
    admission_v1: &mut P,
    emit_v1: F,
) -> Result<(), NativeNormalizationErrorV1<P::Error>>
where
    P: NativeNormalizationAdmissionV1,
    F: FnMut(char) -> Result<(), P::Error>,
{
    let mut data_v1 = NativeNormalizationDataV1::new_v1();
    let _status_v1 = try_for_each_nfc_into_with_native_admission_v1(
        input_v1,
        &mut data_v1,
        admission_v1,
        emit_v1,
    );
    match finish_local_data_v1(data_v1, admission_v1)? {
        NativeNormalizationOutcomeV1::Streamed => Ok(()),
        NativeNormalizationOutcomeV1::IsNfc(_) => {
            Err(NativeNormalizationErrorV1::InvalidNativeProducer)
        }
    }
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
