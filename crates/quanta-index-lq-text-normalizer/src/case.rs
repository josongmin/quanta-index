//! The normalization form and the case fold (see the crate doc).

use std::borrow::Cow;
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
    retained: [usize; 3],
}

impl NfcScratchAdmission {
    fn new(base: usize, ceiling: usize) -> Result<Self, NfcFoldBuildError> {
        if base > ceiling {
            return Err(NfcFoldBuildError::ScratchExceeded);
        }
        Ok(Self {
            base,
            ceiling,
            retained: [0; 3],
        })
    }

    fn slot(owner: NativeNormalizationScratchOwnerV1) -> usize {
        match owner {
            NativeNormalizationScratchOwnerV1::Decomposition => 0,
            NativeNormalizationScratchOwnerV1::Recomposition => 1,
            NativeNormalizationScratchOwnerV1::Sort => 2,
        }
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
        let slot = Self::slot(demand_v1.owner_v1);
        if self.retained[slot] != demand_v1.current_bytes_v1 {
            return Err(NfcAdmissionError::InvalidScratchState);
        }
        let active = self.retained.iter().try_fold(self.base, |sum, bytes| {
            sum.checked_add(*bytes)
                .ok_or(NfcAdmissionError::ArithmeticOverflow)
        })?;
        let peak = active
            .checked_add(demand_v1.new_bytes_v1)
            .ok_or(NfcAdmissionError::ArithmeticOverflow)?;
        if peak > self.ceiling {
            return Err(NfcAdmissionError::ScratchExceeded);
        }
        let success = birth_v1();
        if success {
            self.retained[slot] = demand_v1.new_bytes_v1;
        }
        Ok(success)
    }

    fn release_scratch_v1(&mut self, owner_v1: NativeNormalizationScratchOwnerV1) {
        self.retained[Self::slot(owner_v1)] = 0;
    }
}

/// Exact output lengths for the canonical NFC and per-character lowercase
/// surfaces. The source is borrowed so the plan cannot be applied to other text.
pub struct NfcFoldPlan<'a> {
    source: &'a str,
    nfc_bytes: usize,
    folded_bytes: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub enum NfcFoldBuildError {
    LengthOverflow,
    Allocation,
    LengthChanged,
    ScratchExceeded,
    Native(NativeNormalizationErrorV1<NfcAdmissionError>),
}

impl fmt::Display for NfcFoldBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthOverflow => formatter.write_str("normalization output length overflow"),
            Self::Allocation => formatter.write_str("normalization output allocation refused"),
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
            Self::Native(error) => Some(error),
            _ => None,
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
            .map_err(|_| NfcFoldBuildError::Allocation)?;
        let mut folded = String::new();
        folded
            .try_reserve_exact(self.folded_bytes)
            .map_err(|_| NfcFoldBuildError::Allocation)?;
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

/// The case fold: per-character [`char::to_lowercase`].
#[must_use]
pub fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        out.extend(ch.to_lowercase());
    }
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
    use super::{CaseMode, NfcFoldBuildError, NfcFoldPlan, fold, nfc};

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
        assert_eq!(
            plan.build_with_budget(0, 4),
            Err(NfcFoldBuildError::ScratchExceeded)
        );
    }
}
