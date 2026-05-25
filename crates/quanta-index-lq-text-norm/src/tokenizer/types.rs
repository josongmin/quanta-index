use core::fmt;

/// One token emitted by [`crate::tokenizer::tokenize_text`].
///
/// `surface` is the original byte slice copied verbatim. `lowered` is the
/// case-folded form the writer/reader compares against. `byte_start`/
/// `byte_end` index back into the raw input; they are equal for synthetic
/// identifier sub-parts that overlap their parent token's range.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Token {
    pub surface: Box<str>,
    pub lowered: Box<str>,
    pub kind: TokenKind,
    pub byte_start: u32,
    pub byte_end: u32,
}

/// Token kind. Wire form is `SCREAMING_SNAKE_CASE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Word,
    IdentifierPart,
    Number,
    Punct,
    Operator,
    Comment,
    String,
    RawString,
    Whitespace,
}

impl TokenKind {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Word => "WORD",
            Self::IdentifierPart => "IDENTIFIER_PART",
            Self::Number => "NUMBER",
            Self::Punct => "PUNCT",
            Self::Operator => "OPERATOR",
            Self::Comment => "COMMENT",
            Self::String => "STRING",
            Self::RawString => "RAW_STRING",
            Self::Whitespace => "WHITESPACE",
        }
    }

    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "WORD" => Self::Word,
            "IDENTIFIER_PART" => Self::IdentifierPart,
            "NUMBER" => Self::Number,
            "PUNCT" => Self::Punct,
            "OPERATOR" => Self::Operator,
            "COMMENT" => Self::Comment,
            "STRING" => Self::String,
            "RAW_STRING" => Self::RawString,
            "WHITESPACE" => Self::Whitespace,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for TokenKind {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for TokenKind {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = TokenKind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("TokenKind SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<TokenKind, E> {
                TokenKind::from_code_str(v).ok_or_else(|| E::unknown_variant(v, &["<TokenKind>"]))
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for Token {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("Token", 5)?;
        st.serialize_field("surface", &*self.surface)?;
        st.serialize_field("lowered", &*self.lowered)?;
        st.serialize_field("kind", &self.kind)?;
        st.serialize_field("byte_start", &self.byte_start)?;
        st.serialize_field("byte_end", &self.byte_end)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for Token {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Token;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Token struct with surface/lowered/kind/byte_start/byte_end")
            }
            fn visit_map<A>(self, mut map: A) -> Result<Token, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut surface: Option<String> = None;
                let mut lowered: Option<String> = None;
                let mut kind: Option<TokenKind> = None;
                let mut byte_start: Option<u32> = None;
                let mut byte_end: Option<u32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "surface" => surface = Some(map.next_value()?),
                        "lowered" => lowered = Some(map.next_value()?),
                        "kind" => kind = Some(map.next_value()?),
                        "byte_start" => byte_start = Some(map.next_value()?),
                        "byte_end" => byte_end = Some(map.next_value()?),
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["surface", "lowered", "kind", "byte_start", "byte_end"],
                            ));
                        }
                    }
                }
                Ok(Token {
                    surface: surface
                        .ok_or_else(|| serde::de::Error::missing_field("surface"))?
                        .into_boxed_str(),
                    lowered: lowered
                        .ok_or_else(|| serde::de::Error::missing_field("lowered"))?
                        .into_boxed_str(),
                    kind: kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?,
                    byte_start: byte_start
                        .ok_or_else(|| serde::de::Error::missing_field("byte_start"))?,
                    byte_end: byte_end
                        .ok_or_else(|| serde::de::Error::missing_field("byte_end"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}
