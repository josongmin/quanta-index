//! Case-fold stage.
//!
//! Folds the [`Token::lowered`] field per [`CaseFold`] mode; `surface` is
//! untouched. Idempotent: `fold(fold(t)) == fold(t)`.
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::tokenizer::Token;

/// Case-fold mode for the writer/reader pair.
///
/// `Off` leaves `lowered` exactly as the tokenizer produced it (which is
/// already ASCII-lowered today). `Lower` re-applies ASCII lowercase as a
/// belt-and-braces idempotent step. `NfkcLower` is reserved for the
/// follow-up that pulls in `unicode-normalization`; v1 treats it as
/// `Lower` so the wire shape remains stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaseFold {
    Off,
    Lower,
    NfkcLower,
}

impl CaseFold {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Lower => "LOWER",
            Self::NfkcLower => "NFKC_LOWER",
        }
    }

    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "OFF" => Self::Off,
            "LOWER" => Self::Lower,
            "NFKC_LOWER" => Self::NfkcLower,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for CaseFold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for CaseFold {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for CaseFold {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = CaseFold;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("CaseFold SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<CaseFold, E> {
                CaseFold::from_code_str(v).ok_or_else(|| E::unknown_variant(v, &["<CaseFold>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Apply `mode` to `token`, returning a new token whose `lowered` field
/// reflects the fold. `surface`, `kind`, and byte offsets are preserved.
#[must_use]
pub fn fold_case(token: &Token, mode: CaseFold) -> Token {
    let new_lowered: Box<str> = match mode {
        CaseFold::Off => token.lowered.clone(),
        CaseFold::Lower | CaseFold::NfkcLower => token.lowered.to_lowercase().into_boxed_str(),
    };
    Token {
        surface: token.surface.clone(),
        lowered: new_lowered,
        kind: token.kind,
        byte_start: token.byte_start,
        byte_end: token.byte_end,
    }
}

#[cfg(test)]
mod tests {
    use super::{CaseFold, fold_case};
    use crate::tokenizer::{Token, TokenKind};

    fn t(surface: &str, lowered: &str) -> Token {
        let end = u32::try_from(surface.len()).map_or(u32::MAX, |v| v);
        Token {
            surface: surface.into(),
            lowered: lowered.into(),
            kind: TokenKind::Word,
            byte_start: 0,
            byte_end: end,
        }
    }

    #[test]
    fn fold_off_preserves_lowered() {
        let tok = t("FooBar", "FooBar");
        let out = fold_case(&tok, CaseFold::Off);
        assert_eq!(&*out.lowered, "FooBar");
    }

    #[test]
    fn fold_lower_ascii_uppercase_to_lowercase() {
        let tok = t("FOO", "FOO");
        let out = fold_case(&tok, CaseFold::Lower);
        assert_eq!(&*out.lowered, "foo");
    }

    #[test]
    fn fold_lower_preserves_surface() {
        let tok = t("FOO", "FOO");
        let out = fold_case(&tok, CaseFold::Lower);
        assert_eq!(&*out.surface, "FOO");
    }

    #[test]
    fn fold_is_idempotent() {
        let tok = t("FooBar", "FooBar");
        let a = fold_case(&tok, CaseFold::Lower);
        let b = fold_case(&a, CaseFold::Lower);
        assert_eq!(a, b);
    }

    #[test]
    fn fold_nfkc_lower_lowercases_ascii_today() {
        let tok = t("FOO", "FOO");
        let out = fold_case(&tok, CaseFold::NfkcLower);
        assert_eq!(&*out.lowered, "foo");
    }

    #[test]
    fn code_strs_roundtrip() {
        for m in [CaseFold::Off, CaseFold::Lower, CaseFold::NfkcLower] {
            assert_eq!(CaseFold::from_code_str(m.as_code_str()), Some(m));
        }
    }
}
