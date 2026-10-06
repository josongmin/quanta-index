//! The normalization form and the case fold (see the crate doc).

use std::borrow::Cow;

use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};

/// Exact output lengths for the canonical NFC and per-character lowercase
/// surfaces. The source is borrowed so the plan cannot be applied to other text.
pub struct NfcFoldPlan<'a> {
    source: &'a str,
    nfc_bytes: usize,
    folded_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NfcFoldBuildError {
    LengthOverflow,
    Allocation,
    LengthChanged,
}

impl<'a> NfcFoldPlan<'a> {
    pub fn new(source: &'a str) -> Result<Self, NfcFoldBuildError> {
        let mut nfc_bytes = 0_usize;
        let mut folded_bytes = 0_usize;
        for scalar in source.nfc() {
            nfc_bytes = nfc_bytes
                .checked_add(scalar.len_utf8())
                .ok_or(NfcFoldBuildError::LengthOverflow)?;
            for lower in scalar.to_lowercase() {
                folded_bytes = folded_bytes
                    .checked_add(lower.len_utf8())
                    .ok_or(NfcFoldBuildError::LengthOverflow)?;
            }
        }
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
        let mut indexed = String::new();
        indexed
            .try_reserve_exact(self.nfc_bytes)
            .map_err(|_| NfcFoldBuildError::Allocation)?;
        let mut folded = String::new();
        folded
            .try_reserve_exact(self.folded_bytes)
            .map_err(|_| NfcFoldBuildError::Allocation)?;
        for scalar in self.source.nfc() {
            indexed.push(scalar);
            folded.extend(scalar.to_lowercase());
        }
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
    use super::{CaseMode, fold, nfc};

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
}
