//! First-party NFC producer using only the public Unicode character tables.
//!
//! The scratch policy owns grants; this module owns every buffer it admits.
//! The lexical ingress uses this producer through its existing admission
//! boundary and Q/S identity; supplier migration remains separate work.

use std::fmt;
use std::mem::{size_of, take};

use tinyvec::{Array, TinyVec};
use unicode_normalization::char::{canonical_combining_class, compose, decompose_canonical};

mod admission;
mod prepaid;
mod retained;
pub use NfcError as NativeNormalizationErrorV1;
pub use ScratchOwner as NativeNormalizationScratchOwnerV1;
pub use admission::{
    NativeNormalizationAdmissionV1, NativeNormalizationScratchDemandV1,
    try_for_each_nfc_with_native_admission_v1, try_is_nfc_with_native_admission_v1,
};
pub use prepaid::{PrepaidNfcError, try_is_nfc_with_prepaid_scratch_v1};
pub use retained::{
    NormalizationDataRefusalV1 as NativeNormalizationDataRefusalV1,
    NormalizationDataV1 as NativeNormalizationDataV1,
    NormalizationOutcomeV1 as NativeNormalizationOutcomeV1,
    for_each_nfc_into_v1 as try_for_each_nfc_into_with_native_admission_v1,
    is_nfc_into_v1 as try_is_nfc_into_with_native_admission_v1,
};
pub use retained::{
    NormalizationDataRefusalV1, NormalizationDataV1, NormalizationOutcomeV1, for_each_nfc_into_v1,
    is_nfc_into_v1, is_nfc_unbounded_into_v1,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScratchOwner {
    Decomposition,
    Recomposition,
    Sort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScratchDemand {
    pub owner: ScratchOwner,
    pub current_bytes: usize,
    pub new_bytes: usize,
}

/// `birth` must be called exactly once after admitting both old and new
/// backings. A successful policy retains the new backing until `release`.
pub trait ScratchAdmission {
    type Error;

    fn checkpoint(&mut self, units: u64) -> Result<(), Self::Error>;

    /// Poll an already charged bulk phase without a second debit.
    fn checkpoint_prepaid(&mut self) -> Result<(), Self::Error> {
        self.checkpoint(0)
    }

    fn birth(
        &mut self,
        demand: ScratchDemand,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error>;
    fn release(&mut self, owner: ScratchOwner);
}

#[derive(Debug, PartialEq, Eq)]
pub enum NfcError<E> {
    Admission(E),
    NativeAllocationFailed,
    InvalidNativeProducer,
    InvalidNativeCapacity,
    ArithmeticOverflow,
    UnsupportedNativeSortBacking,
    InputTooLong,
}

impl<E: fmt::Display> fmt::Display for NfcError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(error) => write!(formatter, "normalization admission: {error}"),
            Self::NativeAllocationFailed => {
                formatter.write_str("normalization native allocation failed")
            }
            Self::InvalidNativeProducer => {
                formatter.write_str("normalization native producer is invalid")
            }
            Self::InvalidNativeCapacity => {
                formatter.write_str("normalization native capacity is invalid")
            }
            Self::ArithmeticOverflow => {
                formatter.write_str("normalization demand arithmetic overflow")
            }
            Self::UnsupportedNativeSortBacking => {
                formatter.write_str("normalization sort backing is unsupported")
            }
            Self::InputTooLong => formatter.write_str("normalization input exceeds 512 UTF8 bytes"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for NfcError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(error) => Some(error),
            Self::NativeAllocationFailed
            | Self::InvalidNativeProducer
            | Self::InvalidNativeCapacity
            | Self::ArithmeticOverflow
            | Self::UnsupportedNativeSortBacking
            | Self::InputTooLong => None,
        }
    }
}

#[derive(Default)]
struct ScratchBuffers {
    decomposition: TinyVec<[(u8, char); 4]>,
    sort: Vec<(u8, char)>,
    recomposition: TinyVec<[char; 4]>,
    sort_attempted: bool,
}

impl ScratchBuffers {
    fn release<A: ScratchAdmission>(&mut self, admission: &mut A) {
        // The grant remains live until each backing has actually been dropped.
        drop(take(&mut self.decomposition));
        drop(take(&mut self.recomposition));
        drop(take(&mut self.sort));
        if self.sort_attempted {
            admission.release(ScratchOwner::Sort);
        }
        admission.release(ScratchOwner::Decomposition);
        admission.release(ScratchOwner::Recomposition);
    }
}

struct Scratch<'a, A: ScratchAdmission> {
    admission: &'a mut A,
    decomposition: &'a mut TinyVec<[(u8, char); 4]>,
    sort: &'a mut Vec<(u8, char)>,
    recomposition: &'a mut TinyVec<[char; 4]>,
    sort_attempted: &'a mut bool,
    allow_portable_short_sort: bool,
}

impl<'a, A: ScratchAdmission> Scratch<'a, A> {
    fn new(admission: &'a mut A, buffers: &'a mut ScratchBuffers) -> Self {
        Self {
            admission,
            decomposition: &mut buffers.decomposition,
            sort: &mut buffers.sort,
            recomposition: &mut buffers.recomposition,
            sort_attempted: &mut buffers.sort_attempted,
            allow_portable_short_sort: false,
        }
    }
}

fn push_tiny<T: Array, A: ScratchAdmission>(
    admission: &mut A,
    owner: ScratchOwner,
    buffer: &mut TinyVec<T>,
    item: T::Item,
) -> Result<(), NfcError<A::Error>> {
    if buffer.len() == buffer.capacity() {
        let target = buffer
            .capacity()
            .checked_mul(2)
            .ok_or(NfcError::ArithmeticOverflow)?;
        grow_tiny(admission, owner, buffer, target)?;
    }
    buffer.push(item);
    Ok(())
}

fn grow_tiny<T: Array, A: ScratchAdmission>(
    admission: &mut A,
    owner: ScratchOwner,
    buffer: &mut TinyVec<T>,
    target: usize,
) -> Result<(), NfcError<A::Error>> {
    let current_bytes = if buffer.is_heap() {
        buffer
            .capacity()
            .checked_mul(size_of::<T::Item>())
            .ok_or(NfcError::ArithmeticOverflow)?
    } else {
        0
    };
    let new_bytes = target
        .checked_mul(size_of::<T::Item>())
        .ok_or(NfcError::ArithmeticOverflow)?;
    let additional = target
        .checked_sub(buffer.len())
        .ok_or(NfcError::ArithmeticOverflow)?;
    let mut invoked = false;
    let mut repeated = false;
    let mut native_success = false;
    let accepted = admission
        .birth(
            ScratchDemand {
                owner,
                current_bytes,
                new_bytes,
            },
            &mut || {
                if invoked {
                    repeated = true;
                    return false;
                }
                invoked = true;
                native_success = buffer.try_reserve_exact(additional).is_ok();
                native_success
            },
        )
        .map_err(NfcError::Admission)?;
    if !invoked || repeated || accepted != native_success {
        return Err(NfcError::InvalidNativeProducer);
    }
    if !native_success {
        return Err(NfcError::NativeAllocationFailed);
    }
    if buffer.capacity() != target {
        return Err(NfcError::InvalidNativeCapacity);
    }
    Ok(())
}

fn grow_vec<T, A: ScratchAdmission>(
    admission: &mut A,
    owner: ScratchOwner,
    buffer: &mut Vec<T>,
    target: usize,
    attempted: &mut bool,
) -> Result<(), NfcError<A::Error>> {
    let current_bytes = buffer
        .capacity()
        .checked_mul(size_of::<T>())
        .ok_or(NfcError::ArithmeticOverflow)?;
    let new_bytes = target
        .checked_mul(size_of::<T>())
        .ok_or(NfcError::ArithmeticOverflow)?;
    let additional = target
        .checked_sub(buffer.len())
        .ok_or(NfcError::ArithmeticOverflow)?;
    let mut invoked = false;
    let mut repeated = false;
    let mut native_success = false;
    *attempted = true;
    let accepted = admission
        .birth(
            ScratchDemand {
                owner,
                current_bytes,
                new_bytes,
            },
            &mut || {
                if invoked {
                    repeated = true;
                    return false;
                }
                invoked = true;
                native_success = buffer.try_reserve_exact(additional).is_ok();
                native_success
            },
        )
        .map_err(NfcError::Admission)?;
    if !invoked || repeated || accepted != native_success {
        return Err(NfcError::InvalidNativeProducer);
    }
    if !native_success {
        return Err(NfcError::NativeAllocationFailed);
    }
    if buffer.capacity() != target {
        return Err(NfcError::InvalidNativeCapacity);
    }
    Ok(())
}

fn work<A: ScratchAdmission>(
    scratch: &mut Scratch<'_, A>,
    units: usize,
) -> Result<(), NfcError<A::Error>> {
    let units = u64::try_from(units).map_err(|_error| NfcError::ArithmeticOverflow)?;
    scratch
        .admission
        .checkpoint(units)
        .map_err(NfcError::Admission)
}

const BULK_SCALARS: usize = 512;

fn checkpoint_prepaid<A: ScratchAdmission>(admission: &mut A) -> Result<(), NfcError<A::Error>> {
    admission.checkpoint_prepaid().map_err(NfcError::Admission)
}

fn sort_pending<A: ScratchAdmission>(
    scratch: &mut Scratch<'_, A>,
    start: usize,
) -> Result<(), NfcError<A::Error>> {
    let len = scratch
        .decomposition
        .len()
        .checked_sub(start)
        .ok_or(NfcError::InvalidNativeProducer)?;
    work(scratch, len)?;
    if len <= 512 {
        // The pinned 64-bit Rust 1.92 stable sort keeps <=512 pairs on the
        // stack. This is the same platform boundary as the current producer.
        let pairs = scratch
            .decomposition
            .get_mut(start..)
            .ok_or(NfcError::InvalidNativeProducer)?;
        if cfg!(all(
            target_pointer_width = "64",
            any(target_os = "linux", target_os = "macos")
        )) && size_of::<(u8, char)>() == 8
        {
            pairs.sort_by_key(|pair| pair.0);
        } else if scratch.allow_portable_short_sort {
            // The prepaid borrowed protocol covers a quadratic pass. A
            // stable insertion sort on at most 512 entries needs no backing.
            for index in 1..pairs.len() {
                checkpoint_prepaid(scratch.admission)?;
                let mut current = index;
                while let Some(previous) = current.checked_sub(1) {
                    let previous_class = pairs
                        .get(previous)
                        .ok_or(NfcError::InvalidNativeProducer)?
                        .0;
                    let current_class =
                        pairs.get(current).ok_or(NfcError::InvalidNativeProducer)?.0;
                    if previous_class <= current_class {
                        break;
                    }
                    pairs.swap(previous, current);
                    current = previous;
                }
            }
        } else {
            return Err(NfcError::UnsupportedNativeSortBacking);
        }
        return Ok(());
    }
    let sort_work = len.checked_mul(3).ok_or(NfcError::ArithmeticOverflow)?;
    work(scratch, sort_work)?;
    if scratch.sort.capacity() < len {
        grow_vec(
            scratch.admission,
            ScratchOwner::Sort,
            scratch.sort,
            len,
            scratch.sort_attempted,
        )?;
    }
    scratch.sort.clear();
    while scratch.sort.len() < len {
        checkpoint_prepaid(scratch.admission)?;
        let additional = len
            .checked_sub(scratch.sort.len())
            .ok_or(NfcError::ArithmeticOverflow)?
            .min(BULK_SCALARS);
        let target = scratch
            .sort
            .len()
            .checked_add(additional)
            .ok_or(NfcError::ArithmeticOverflow)?;
        scratch.sort.resize(target, (0, '\0'));
    }
    let pairs = scratch
        .decomposition
        .get_mut(start..)
        .ok_or(NfcError::InvalidNativeProducer)?;
    let mut positions = [0_usize; 256];
    for chunk in pairs.chunks(BULK_SCALARS) {
        checkpoint_prepaid(scratch.admission)?;
        for &(class, _) in chunk {
            let position = positions
                .get_mut(usize::from(class))
                .ok_or(NfcError::InvalidNativeProducer)?;
            *position = position
                .checked_add(1)
                .ok_or(NfcError::ArithmeticOverflow)?;
        }
    }
    let mut next = 0;
    for position in &mut positions {
        let count = *position;
        *position = next;
        next = next
            .checked_add(count)
            .ok_or(NfcError::ArithmeticOverflow)?;
    }
    for chunk in pairs.chunks(BULK_SCALARS) {
        checkpoint_prepaid(scratch.admission)?;
        for &pair in chunk {
            let position = positions
                .get_mut(usize::from(pair.0))
                .ok_or(NfcError::InvalidNativeProducer)?;
            *scratch
                .sort
                .get_mut(*position)
                .ok_or(NfcError::InvalidNativeProducer)? = pair;
            *position = position
                .checked_add(1)
                .ok_or(NfcError::ArithmeticOverflow)?;
        }
    }
    for (target, sorted) in pairs
        .chunks_mut(BULK_SCALARS)
        .zip(scratch.sort.chunks(BULK_SCALARS))
    {
        checkpoint_prepaid(scratch.admission)?;
        target.copy_from_slice(sorted);
    }
    Ok(())
}

struct Decomposition<'a> {
    input: std::str::Chars<'a>,
    ready: std::ops::Range<usize>,
}

fn push_back<A: ScratchAdmission>(
    scratch: &mut Scratch<'_, A>,
    state: &mut Decomposition<'_>,
    scalar: char,
) -> Result<(), NfcError<A::Error>> {
    work(scratch, 1)?;
    let class = canonical_combining_class(scalar);
    if class == 0 {
        sort_pending(scratch, state.ready.end)?;
    }
    push_tiny(
        scratch.admission,
        ScratchOwner::Decomposition,
        scratch.decomposition,
        (class, scalar),
    )?;
    if class == 0 {
        state.ready.end = scratch.decomposition.len();
    }
    Ok(())
}

fn reset_decomposition<A: ScratchAdmission>(
    scratch: &mut Scratch<'_, A>,
    state: &mut Decomposition<'_>,
) -> Result<(), NfcError<A::Error>> {
    let pending = scratch
        .decomposition
        .len()
        .checked_sub(state.ready.end)
        .ok_or(NfcError::InvalidNativeProducer)?;
    work(scratch, pending)?;
    // The next starter completes the ready run. At most the remaining
    // parts of that one decomposed scalar follow it. At EOF pending is empty.
    scratch.decomposition.copy_within(state.ready.end.., 0);
    scratch.decomposition.truncate(pending);
    state.ready = 0..0;
    Ok(())
}

fn next_decomposed<A: ScratchAdmission>(
    scratch: &mut Scratch<'_, A>,
    state: &mut Decomposition<'_>,
) -> Result<Option<char>, NfcError<A::Error>> {
    while state.ready.end == 0 {
        work(scratch, 1)?;
        if let Some(scalar) = state.input.next() {
            work(scratch, 1)?;
            let mut failure = None;
            decompose_canonical(scalar, |part| {
                if failure.is_none() {
                    failure = push_back(scratch, state, part).err();
                }
            });
            if let Some(cause) = failure {
                return Err(cause);
            }
        } else if scratch.decomposition.is_empty() {
            return Ok(None);
        } else {
            sort_pending(scratch, state.ready.end)?;
            state.ready.end = scratch.decomposition.len();
            break;
        }
    }
    let scalar = scratch
        .decomposition
        .get(state.ready.start)
        .ok_or(NfcError::InvalidNativeProducer)?
        .1;
    let next = state
        .ready
        .start
        .checked_add(1)
        .ok_or(NfcError::ArithmeticOverflow)?;
    if next == state.ready.end {
        reset_decomposition(scratch, state)?;
    } else {
        state.ready.start = next;
    }
    Ok(Some(scalar))
}

#[derive(Default)]
struct Recomposition {
    phase: RecompositionPhase,
    composee: Option<char>,
    last_class: Option<u8>,
}

#[derive(Default)]
enum RecompositionPhase {
    #[default]
    Composing,
    Purging(usize),
    Finished(usize),
}

fn next_composed<A: ScratchAdmission>(
    scratch: &mut Scratch<'_, A>,
    decomposition: &mut Decomposition<'_>,
    state: &mut Recomposition,
) -> Result<Option<char>, NfcError<A::Error>> {
    loop {
        work(scratch, 1)?;
        match state.phase {
            RecompositionPhase::Composing => {
                while let Some(scalar) = next_decomposed(scratch, decomposition)? {
                    work(scratch, 1)?;
                    let class = canonical_combining_class(scalar);
                    let Some(starter) = state.composee else {
                        if class != 0 {
                            return Ok(Some(scalar));
                        }
                        state.composee = Some(scalar);
                        continue;
                    };
                    match state.last_class {
                        None => match compose(starter, scalar) {
                            Some(composed) => {
                                state.composee = Some(composed);
                            }
                            None if class == 0 => {
                                state.composee = Some(scalar);
                                return Ok(Some(starter));
                            }
                            None => {
                                push_tiny(
                                    scratch.admission,
                                    ScratchOwner::Recomposition,
                                    scratch.recomposition,
                                    scalar,
                                )?;
                                state.last_class = Some(class);
                            }
                        },
                        Some(previous) if previous >= class => {
                            if class == 0 {
                                state.composee = Some(scalar);
                                state.last_class = None;
                                state.phase = RecompositionPhase::Purging(0);
                                return Ok(Some(starter));
                            }
                            push_tiny(
                                scratch.admission,
                                ScratchOwner::Recomposition,
                                scratch.recomposition,
                                scalar,
                            )?;
                            state.last_class = Some(class);
                        }
                        Some(_) => {
                            if let Some(composed) = compose(starter, scalar) {
                                state.composee = Some(composed);
                            } else {
                                push_tiny(
                                    scratch.admission,
                                    ScratchOwner::Recomposition,
                                    scratch.recomposition,
                                    scalar,
                                )?;
                                state.last_class = Some(class);
                            }
                        }
                    }
                }
                state.phase = RecompositionPhase::Finished(0);
                if state.composee.is_some() {
                    return Ok(state.composee.take());
                }
            }
            RecompositionPhase::Purging(index) => {
                if let Some(&scalar) = scratch.recomposition.get(index) {
                    state.phase = RecompositionPhase::Purging(
                        index.checked_add(1).ok_or(NfcError::ArithmeticOverflow)?,
                    );
                    return Ok(Some(scalar));
                }
                scratch.recomposition.clear();
                state.phase = RecompositionPhase::Composing;
            }
            RecompositionPhase::Finished(index) => {
                if let Some(&scalar) = scratch.recomposition.get(index) {
                    state.phase = RecompositionPhase::Finished(
                        index.checked_add(1).ok_or(NfcError::ArithmeticOverflow)?,
                    );
                    return Ok(Some(scalar));
                }
                scratch.recomposition.clear();
                return Ok(state.composee.take());
            }
        }
    }
}

/// Emit NFC scalars using only public Unicode character data. The work and
/// scratch state machines mirror the qualified controlled iterator contract.
pub fn for_each_nfc<A, F>(
    source: &str,
    admission: &mut A,
    emit: F,
) -> Result<(), NfcError<A::Error>>
where
    A: ScratchAdmission,
    F: FnMut(char) -> Result<(), A::Error>,
{
    let mut buffers = ScratchBuffers::default();
    let result = {
        let mut scratch = Scratch::new(admission, &mut buffers);
        stream_nfc(source, &mut scratch, emit)
    };
    // On unwind the original DATA owner drops its backing without invoking
    // release callbacks; the caller retains the funding bank during unwind.
    buffers.release(admission);
    result
}

fn stream_nfc<A, F>(
    source: &str,
    scratch: &mut Scratch<'_, A>,
    mut emit: F,
) -> Result<(), NfcError<A::Error>>
where
    A: ScratchAdmission,
    F: FnMut(char) -> Result<(), A::Error>,
{
    work(scratch, 0)?;
    let mut decomposition = Decomposition {
        input: source.chars(),
        ready: 0..0,
    };
    let mut recomposition = Recomposition::default();
    while let Some(scalar) = next_composed(scratch, &mut decomposition, &mut recomposition)? {
        emit(scalar).map_err(NfcError::Admission)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
