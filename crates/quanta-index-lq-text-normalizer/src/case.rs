//! The normalization form and the case fold (see the crate doc).

use std::borrow::Cow;
use std::collections::TryReserveError;
use std::fmt;

use unicode_normalization::{
    IsNormalized, NativeNormalizationAdmissionV1, NativeNormalizationErrorV1,
    NativeNormalizationScratchDemandV1, NativeNormalizationScratchOwnerV1, UnicodeNormalization,
    is_nfc_quick, try_for_each_nfc_with_native_admission_v1,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NfcAdmissionError {
    ArithmeticOverflow,
    ScratchExceeded,
    InvalidScratchState,
}

impl fmt::Display for NfcAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ArithmeticOverflow => "normalization scratch arithmetic overflow",
            Self::ScratchExceeded => "normalization scratch exceeds policy",
            Self::InvalidScratchState => "normalization scratch custody mismatch",
        })
    }
}

impl std::error::Error for NfcAdmissionError {}

struct NfcScratchAdmission {
    base: usize,
    ceiling: usize,
    retained: NfcRetainedScratch,
}

#[derive(Default)]
struct NfcRetainedScratch {
    decomposition: usize,
    recomposition: usize,
    sort: usize,
}

impl NfcRetainedScratch {
    fn get(&self, owner: NativeNormalizationScratchOwnerV1) -> usize {
        match owner {
            NativeNormalizationScratchOwnerV1::Decomposition => self.decomposition,
            NativeNormalizationScratchOwnerV1::Recomposition => self.recomposition,
            NativeNormalizationScratchOwnerV1::Sort => self.sort,
        }
    }

    fn get_mut(&mut self, owner: NativeNormalizationScratchOwnerV1) -> &mut usize {
        match owner {
            NativeNormalizationScratchOwnerV1::Decomposition => &mut self.decomposition,
            NativeNormalizationScratchOwnerV1::Recomposition => &mut self.recomposition,
            NativeNormalizationScratchOwnerV1::Sort => &mut self.sort,
        }
    }

    fn checked_total(&self, base: usize) -> Result<usize, NfcAdmissionError> {
        [self.decomposition, self.recomposition, self.sort]
            .into_iter()
            .try_fold(base, |sum, bytes| {
                sum.checked_add(bytes)
                    .ok_or(NfcAdmissionError::ArithmeticOverflow)
            })
    }
}

impl NfcScratchAdmission {
    fn new(base: usize, ceiling: usize) -> Result<Self, NfcFoldBuildError> {
        if base > ceiling {
            return Err(NfcFoldBuildError::ScratchExceeded);
        }
        Ok(Self {
            base,
            ceiling,
            retained: NfcRetainedScratch::default(),
        })
    }
}

impl NativeNormalizationAdmissionV1 for NfcScratchAdmission {
    type Error = NfcAdmissionError;

    fn checkpoint_work_v1(&mut self, _units_v1: u64) -> Result<(), Self::Error> {
        Ok(())
    }

    fn native_birth_v1(
        &mut self,
        demand_v1: NativeNormalizationScratchDemandV1,
        birth_v1: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error> {
        if self.retained.get(demand_v1.owner_v1) != demand_v1.current_bytes_v1 {
            return Err(NfcAdmissionError::InvalidScratchState);
        }
        let active = self.retained.checked_total(self.base)?;
        let peak = active
            .checked_add(demand_v1.new_bytes_v1)
            .ok_or(NfcAdmissionError::ArithmeticOverflow)?;
        if peak > self.ceiling {
            return Err(NfcAdmissionError::ScratchExceeded);
        }
        let success = birth_v1();
        if success {
            *self.retained.get_mut(demand_v1.owner_v1) = demand_v1.new_bytes_v1;
        }
        Ok(success)
    }

    fn release_scratch_v1(&mut self, owner_v1: NativeNormalizationScratchOwnerV1) {
        *self.retained.get_mut(owner_v1) = 0;
    }
}

/// Exact output lengths for the canonical NFC and per-character lowercase
/// surfaces. The source is borrowed so the plan cannot be applied to other text.
pub struct NfcFoldPlan<'a> {
    source: &'a str,
    nfc_bytes: usize,
    folded_bytes: usize,
}

#[derive(Debug)]
pub enum NfcFoldBuildError {
    LengthOverflow,
    Allocation {
        surface: &'static str,
        source: TryReserveError,
    },
    LengthChanged,
    ScratchExceeded,
    Native(NativeNormalizationErrorV1<NfcAdmissionError>),
}

impl fmt::Display for NfcFoldBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthOverflow => formatter.write_str("normalization output length overflow"),
            Self::Allocation { surface, source } => {
                write!(
                    formatter,
                    "normalization {surface} allocation refused: {source}"
                )
            }
            Self::LengthChanged => formatter.write_str("normalization output length changed"),
            Self::ScratchExceeded => {
                formatter.write_str("normalization output exceeds scratch policy")
            }
            Self::Native(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for NfcFoldBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Allocation { source, .. } => Some(source),
            Self::Native(error) => Some(error),
            Self::LengthOverflow | Self::LengthChanged | Self::ScratchExceeded => None,
        }
    }
}

impl<'a> NfcFoldPlan<'a> {
    pub fn new(source: &'a str) -> Result<Self, NfcFoldBuildError> {
        Self::new_with_budget(source, 0, usize::MAX)
    }

    pub fn new_with_budget(
        source: &'a str,
        base: usize,
        ceiling: usize,
    ) -> Result<Self, NfcFoldBuildError> {
        let mut admission = NfcScratchAdmission::new(base, ceiling)?;
        let mut nfc_bytes = 0_usize;
        let mut folded_bytes = 0_usize;
        try_for_each_nfc_with_native_admission_v1(source, &mut admission, |scalar| {
            nfc_bytes = nfc_bytes
                .checked_add(scalar.len_utf8())
                .ok_or(NfcAdmissionError::ArithmeticOverflow)?;
            for lower in scalar.to_lowercase() {
                folded_bytes = folded_bytes
                    .checked_add(lower.len_utf8())
                    .ok_or(NfcAdmissionError::ArithmeticOverflow)?;
            }
            Ok(())
        })
        .map_err(NfcFoldBuildError::Native)?;
        Ok(Self {
            source,
            nfc_bytes,
            folded_bytes,
        })
    }

    #[must_use]
    pub fn nfc_bytes(&self) -> usize {
        self.nfc_bytes
    }

    #[must_use]
    pub fn folded_bytes(&self) -> usize {
        self.folded_bytes
    }

    /// Call only after the owner admits both exact lengths. Allocation is
    /// fallible, and either output is discarded on any failure.
    pub fn build(self) -> Result<(String, String), NfcFoldBuildError> {
        self.build_with_budget(0, usize::MAX)
    }

    pub fn build_with_budget(
        self,
        base: usize,
        ceiling: usize,
    ) -> Result<(String, String), NfcFoldBuildError> {
        let output_bytes = self
            .nfc_bytes
            .checked_add(self.folded_bytes)
            .ok_or(NfcFoldBuildError::LengthOverflow)?;
        let admitted_base = base
            .checked_add(output_bytes)
            .ok_or(NfcFoldBuildError::LengthOverflow)?;
        let mut admission = NfcScratchAdmission::new(admitted_base, ceiling)?;
        let mut indexed = String::new();
        indexed
            .try_reserve_exact(self.nfc_bytes)
            .map_err(|source| NfcFoldBuildError::Allocation {
                surface: "NFC",
                source,
            })?;
        let mut folded = String::new();
        folded
            .try_reserve_exact(self.folded_bytes)
            .map_err(|source| NfcFoldBuildError::Allocation {
                surface: "folded",
                source,
            })?;
        try_for_each_nfc_with_native_admission_v1(self.source, &mut admission, |scalar| {
            indexed.push(scalar);
            folded.extend(scalar.to_lowercase());
            Ok(())
        })
        .map_err(NfcFoldBuildError::Native)?;
        if indexed.len() != self.nfc_bytes || folded.len() != self.folded_bytes {
            return Err(NfcFoldBuildError::LengthChanged);
        }
        Ok((indexed, folded))
    }
}

/// Failure of bounded, cancellable NFC production.
#[derive(Debug)]
pub enum BoundedNfcError<E> {
    Checkpoint(E),
    Memory(E),
    OutputExceeded,
    Allocation(TryReserveError),
    LengthChanged,
    Native(NativeNormalizationErrorV1<BoundedNfcAdmissionError<E>>),
}

#[derive(Debug)]
pub enum BoundedNfcAdmissionError<E> {
    Checkpoint(E),
    Memory(E),
    Scratch(NfcAdmissionError),
    OutputExceeded,
    LengthOverflow,
}

impl<E: fmt::Display> fmt::Display for BoundedNfcAdmissionError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Checkpoint(error) | Self::Memory(error) => fmt::Display::fmt(error, formatter),
            Self::Scratch(error) => fmt::Display::fmt(error, formatter),
            Self::OutputExceeded => {
                formatter.write_str("normalization output exceeds request limit")
            }
            Self::LengthOverflow => formatter.write_str("normalization output length overflow"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for BoundedNfcAdmissionError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Checkpoint(error) | Self::Memory(error) => Some(error),
            Self::Scratch(error) => Some(error),
            Self::OutputExceeded | Self::LengthOverflow => None,
        }
    }
}

impl<E: fmt::Display> fmt::Display for BoundedNfcError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Checkpoint(error) | Self::Memory(error) => fmt::Display::fmt(error, formatter),
            Self::OutputExceeded => {
                formatter.write_str("normalization output exceeds request limit")
            }
            Self::Allocation(error) => write!(
                formatter,
                "normalization output allocation refused: {error}"
            ),
            Self::LengthChanged => formatter.write_str("normalization output length changed"),
            Self::Native(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for BoundedNfcError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Checkpoint(error) | Self::Memory(error) => Some(error),
            Self::Allocation(error) => Some(error),
            Self::Native(error) => Some(error),
            Self::OutputExceeded | Self::LengthChanged => None,
        }
    }
}

impl<E> BoundedNfcError<E> {
    /// Recover the caller's exact work or memory admission refusal.
    pub fn into_checkpoint(self) -> Result<E, Self> {
        match self {
            Self::Checkpoint(error)
            | Self::Memory(error)
            | Self::Native(NativeNormalizationErrorV1::Admission(
                BoundedNfcAdmissionError::Checkpoint(error)
                | BoundedNfcAdmissionError::Memory(error),
            )) => Ok(error),
            other @ (Self::OutputExceeded
            | Self::Allocation(_)
            | Self::LengthChanged
            | Self::Native(_)) => Err(other),
        }
    }
}

struct BoundedNfcAdmission<'a, F, R, L> {
    scratch: NfcScratchAdmission,
    checkpoint: &'a mut F,
    reserve: &'a mut R,
    leases: NfcScratchLeases<L>,
    pending_work: u64,
}

struct NfcScratchLeases<L> {
    decomposition: Option<L>,
    recomposition: Option<L>,
    sort: Option<L>,
}

impl<L> NfcScratchLeases<L> {
    fn new() -> Self {
        Self {
            decomposition: None,
            recomposition: None,
            sort: None,
        }
    }

    fn get_mut(&mut self, owner: NativeNormalizationScratchOwnerV1) -> &mut Option<L> {
        match owner {
            NativeNormalizationScratchOwnerV1::Decomposition => &mut self.decomposition,
            NativeNormalizationScratchOwnerV1::Recomposition => &mut self.recomposition,
            NativeNormalizationScratchOwnerV1::Sort => &mut self.sort,
        }
    }
}

impl<F, R, L, E> NativeNormalizationAdmissionV1 for BoundedNfcAdmission<'_, F, R, L>
where
    F: FnMut(u64) -> Result<(), E>,
    R: FnMut(usize) -> Result<L, E>,
{
    type Error = BoundedNfcAdmissionError<E>;

    fn checkpoint_work_v1(&mut self, units_v1: u64) -> Result<(), Self::Error> {
        self.pending_work = self
            .pending_work
            .checked_add(units_v1)
            .ok_or(BoundedNfcAdmissionError::LengthOverflow)?;
        if self.pending_work >= 1024 {
            let units = std::mem::take(&mut self.pending_work);
            (self.checkpoint)(units).map_err(BoundedNfcAdmissionError::Checkpoint)?;
        }
        Ok(())
    }

    fn native_birth_v1(
        &mut self,
        demand_v1: NativeNormalizationScratchDemandV1,
        birth_v1: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::Error> {
        let mut refusal = None;
        let mut replacement = None;
        let result = self
            .scratch
            .native_birth_v1(
                demand_v1,
                &mut || match (self.reserve)(demand_v1.new_bytes_v1) {
                    Ok(lease) => {
                        if birth_v1() {
                            replacement = Some(lease);
                            true
                        } else {
                            false
                        }
                    }
                    Err(error) => {
                        refusal = Some(error);
                        false
                    }
                },
            )
            .map_err(BoundedNfcAdmissionError::Scratch)?;
        if let Some(error) = refusal {
            return Err(BoundedNfcAdmissionError::Memory(error));
        }
        if result {
            // Old and replacement grants coexist through native reallocation.
            *self.leases.get_mut(demand_v1.owner_v1) = replacement;
        }
        Ok(result)
    }

    fn release_scratch_v1(&mut self, owner_v1: NativeNormalizationScratchOwnerV1) {
        self.scratch.release_scratch_v1(owner_v1);
        *self.leases.get_mut(owner_v1) = None;
    }
}

/// Count exact NFC output without allocating it. Borrow canonical input.
///
/// `reserve` must return a grant covering the requested bytes until it drops.
/// Scratch grants cover native birth/growth and end after backing is dropped.
/// The returned output grant must remain with the owned text through its use.
pub fn bounded_nfc_with_checkpoint<E, L>(
    source: &str,
    max_output_bytes: usize,
    max_total_bytes: usize,
    mut checkpoint: impl FnMut(u64) -> Result<(), E>,
    mut reserve: impl FnMut(usize) -> Result<L, E>,
) -> Result<(Cow<'_, str>, Option<L>), BoundedNfcError<E>> {
    checkpoint(0).map_err(BoundedNfcError::Checkpoint)?;
    let mut output_bytes = 0usize;
    let mut original = source.chars();
    let mut unchanged = true;
    {
        let mut admission = BoundedNfcAdmission {
            scratch: NfcScratchAdmission::new(0, max_total_bytes)
                .map_err(|_error| BoundedNfcError::OutputExceeded)?,
            checkpoint: &mut checkpoint,
            reserve: &mut reserve,
            leases: NfcScratchLeases::new(),
            pending_work: 0,
        };
        try_for_each_nfc_with_native_admission_v1(source, &mut admission, |scalar| {
            output_bytes = output_bytes
                .checked_add(scalar.len_utf8())
                .ok_or(BoundedNfcAdmissionError::LengthOverflow)?;
            if output_bytes > max_output_bytes {
                return Err(BoundedNfcAdmissionError::OutputExceeded);
            }
            if original.next() != Some(scalar) {
                unchanged = false;
            }
            Ok(())
        })
        .map_err(BoundedNfcError::Native)?;
    }
    checkpoint(0).map_err(BoundedNfcError::Checkpoint)?;
    if unchanged && original.next().is_none() {
        return Ok((Cow::Borrowed(source), None));
    }
    if output_bytes > max_total_bytes {
        return Err(BoundedNfcError::OutputExceeded);
    }
    // Fund the requested output backing before its fallible allocation.
    let _funded = NfcScratchAdmission::new(output_bytes, max_total_bytes)
        .map_err(|_error| BoundedNfcError::OutputExceeded)?;
    let output_memory = if output_bytes == 0 {
        None
    } else {
        Some(reserve(output_bytes).map_err(BoundedNfcError::Memory)?)
    };
    let mut output = String::new();
    output
        .try_reserve_exact(output_bytes)
        .map_err(BoundedNfcError::Allocation)?;
    {
        let mut admission = BoundedNfcAdmission {
            scratch: NfcScratchAdmission::new(output.capacity(), max_total_bytes)
                .map_err(|_error| BoundedNfcError::OutputExceeded)?,
            checkpoint: &mut checkpoint,
            reserve: &mut reserve,
            leases: NfcScratchLeases::new(),
            pending_work: 0,
        };
        try_for_each_nfc_with_native_admission_v1(source, &mut admission, |scalar| {
            output.push(scalar);
            Ok(())
        })
        .map_err(BoundedNfcError::Native)?;
    }
    checkpoint(0).map_err(BoundedNfcError::Checkpoint)?;
    if output.len() != output_bytes {
        return Err(BoundedNfcError::LengthChanged);
    }
    Ok((Cow::Owned(output), output_memory))
}

/// Whether a surface compares text after the case fold.
///
/// The DSL's `case:` option selects it: `case:yes` is [`CaseMode::Sensitive`],
/// `case:no` — and an absent option, the DSL default — is
/// [`CaseMode::Folded`]. That mapping lives in one place
/// (`quanta_index_lq_norm::LqOptions::case_mode`), so every route reads
/// the default the same way.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CaseMode {
    /// `case:yes`: no fold.
    Sensitive,
    /// `case:no` (the default): per-character Unicode lowercase on both
    /// sides.
    Folded,
}

impl CaseMode {
    #[must_use]
    pub const fn from_case_sensitive(case_sensitive: bool) -> Self {
        if case_sensitive {
            Self::Sensitive
        } else {
            Self::Folded
        }
    }

    /// Stable short label for explain traces and manifests.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sensitive => "sensitive",
            Self::Folded => "folded",
        }
    }
}

/// Unicode NFC of `text`, borrowing when the input is already normalized.
#[must_use]
pub fn nfc(text: &str) -> Cow<'_, str> {
    match is_nfc_quick(text.chars()) {
        IsNormalized::Yes => Cow::Borrowed(text),
        IsNormalized::No | IsNormalized::Maybe => Cow::Owned(text.nfc().collect()),
    }
}

enum CaseScalar {
    Sensitive(std::iter::Once<char>),
    Folded(std::char::ToLowercase),
}

impl Iterator for CaseScalar {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self {
            Self::Sensitive(chars) => chars.next(),
            Self::Folded(chars) => chars.next(),
        }
    }
}

/// Stream the canonical case surface without allocating an intermediate string.
pub fn case_chars(text: &str, case: CaseMode) -> impl Iterator<Item = char> + '_ {
    text.chars().flat_map(move |ch| match case {
        CaseMode::Sensitive => CaseScalar::Sensitive(std::iter::once(ch)),
        CaseMode::Folded => CaseScalar::Folded(ch.to_lowercase()),
    })
}

/// The case fold: per-character [`char::to_lowercase`].
#[must_use]
pub fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    out.extend(case_chars(text, CaseMode::Folded));
    out
}

/// Apply the case mode to already-NFC text.
#[must_use]
pub fn apply_case(text: &str, case: CaseMode) -> Cow<'_, str> {
    match case {
        CaseMode::Sensitive => Cow::Borrowed(text),
        CaseMode::Folded => Cow::Owned(fold(text)),
    }
}

#[cfg(test)]
mod tests {
    use super::{CaseMode, NfcFoldBuildError, NfcFoldPlan, case_chars, fold, nfc};

    #[test]
    fn streamed_case_surfaces_match_fixed_unicode_gold() {
        for (input, folded) in [
            ("", ""),
            ("ASCII", "ascii"),
            ("CAFÉ", "café"),
            ("ΟΔΟΣ", "οδοσ"),
            ("Straße", "straße"),
            ("İ", "i\u{307}"),
        ] {
            assert_eq!(
                case_chars(input, CaseMode::Sensitive).collect::<String>(),
                input
            );
            assert_eq!(
                case_chars(input, CaseMode::Folded).collect::<String>(),
                folded
            );
        }
    }

    #[test]
    fn fold_is_unicode_lowercase_per_char() {
        assert_eq!(fold("CAFÉ"), "café");
        assert_eq!(fold("Данные"), "данные");
        assert_eq!(fold("ΣΊΣΥΦΟΣ"), "σίσυφοσ");
        assert_eq!(fold("ΟΔΟΣ"), "οδοσ");
        assert_ne!(fold("ΟΔΟΣ"), "ΟΔΟΣ".to_lowercase(), "no final-sigma rule");
        assert_eq!(fold("Straße"), "straße");
        assert_eq!(fold("İ"), "i\u{307}");
    }

    #[test]
    fn nfc_composes_and_maps_singletons() {
        assert_eq!(nfc("cafe\u{301}"), "café");
        assert_eq!(nfc("\u{212A}elvin"), "Kelvin");
        assert_eq!(nfc("ｆｏｏ"), "ｆｏｏ");
        assert!(matches!(nfc("plain"), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn the_default_mode_is_folded() {
        assert_eq!(CaseMode::from_case_sensitive(false), CaseMode::Folded);
        assert_eq!(CaseMode::from_case_sensitive(true), CaseMode::Sensitive);
        assert_eq!(CaseMode::Folded.as_str(), "folded");
    }

    #[test]
    fn planned_nfc_and_fold_preserve_fixed_unicode_outputs() {
        let source = format!("a{}\u{300}", "\u{315}".repeat(513));
        let expected = format!("à{}", "\u{315}".repeat(513));
        let plan = NfcFoldPlan::new_with_budget(&source, 0, 1 << 20).expect("admitted census");
        assert_eq!(plan.nfc_bytes(), expected.len());
        assert_eq!(plan.folded_bytes(), expected.len());
        let (indexed, folded) = plan.build_with_budget(0, 1 << 20).expect("admitted build");
        assert_eq!(indexed, expected);
        assert_eq!(folded, expected);
    }

    #[test]
    fn planned_normalization_refuses_scratch_before_output_birth() {
        let source = format!("a{}", "\u{315}".repeat(513));
        assert!(NfcFoldPlan::new_with_budget(&source, 0, 1).is_err());
        let plan = NfcFoldPlan::new("İ").expect("census");
        assert_eq!(plan.nfc_bytes(), 2);
        assert_eq!(plan.folded_bytes(), 3);
        assert!(matches!(
            plan.build_with_budget(0, 4),
            Err(NfcFoldBuildError::ScratchExceeded)
        ));
    }
}
