// Internal canonical AST.
//
// PRE-NORM owns this shape until PRE-CONTRACT-EXT lands the public `LqQuery`
// carrier in the contract crate; at that point the integration ticket rewires
// `IntoLqQuery for LqNormalizedQuery`. The shape here covers the subset of
// dsl.md §2 that PRE-NORM actually executes: boolean layer, pattern leaves,
// filters, directives, structural-block leaf, and the `LqOptions` knobs
// touched by normalization (case, patterntype, count).
//
// D18: every serde impl on this module is hand-rolled.

use core::fmt;

use crate::errors::LqSpan;

/// Lq DSL version tag baked into the canonical hash root.
/// Pre-freeze tag per ticket §5 step 39 / dsl.md §11.1; bumps to "1.0" in
/// LEX-01 once the carrier ships.
pub const LQ_VERSION_TAG: &str = "1.0-pre";

/// Pattern-type mode from dsl.md §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqPatternType {
    Literal,
    Keyword,
    Standard,
    Regexp,
    Structural,
}

impl LqPatternType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Keyword => "keyword",
            Self::Standard => "standard",
            Self::Regexp => "regexp",
            Self::Structural => "structural",
        }
    }
}

/// `case:` option per dsl.md §6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqCase {
    Sensitive,
    Insensitive,
}

/// `count:` option per dsl.md §6.2 / §13. Bounded values are cap-checked at
/// parse against `MAX_COUNT_BOUNDED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqCountBound {
    Bounded(u32),
    All,
}

/// `select:` projection dimension per dsl.md §6.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqSelect {
    Repo,
    File,
    Path,
    Symbol,
    Content,
    ContentMatch,
}

impl LqSelect {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::File => "file",
            Self::Path => "path",
            Self::Symbol => "symbol",
            Self::Content => "content",
            Self::ContentMatch => "content.match",
        }
    }
}

/// Per dsl.md §6.2 path/file scope tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqFileScope {
    NameAndPath,
    PathOnly,
}

impl LqFileScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NameAndPath => "name_and_path",
            Self::PathOnly => "path_only",
        }
    }
}

/// `type:` value per dsl.md §6.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqType {
    File,
    Path,
    Symbol,
    Commit,
    Diff,
    Repo,
}

impl LqType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Path => "path",
            Self::Symbol => "symbol",
            Self::Commit => "commit",
            Self::Diff => "diff",
            Self::Repo => "repo",
        }
    }
}

/// Pattern leaf shape per dsl.md §3 / §1.5.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqLeaf {
    /// Bare keyword token.
    Keyword(String),
    /// `"..."` phrase, escapes decoded.
    Phrase(String),
    /// `'...'` raw string, escapes decoded.
    RawString(String),
    /// `/.../` regex source (post-`(?i)`-stripping if any).
    Regex(String),
    /// `match { ... }` body raw text — full structural sub-grammar
    /// (dsl.md §8) is deferred; PRE-NORM ships the leaf carrier with the
    /// raw body and node-count guard. Body text excludes the surrounding
    /// `match {` / `}`.
    StructuralBlock(String),
}

/// Filter node per dsl.md §6.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqFilter {
    Repo { pattern: String, revs: Vec<String> },
    File { pattern: String, scope: LqFileScope },
    Lang { id: String },
    Rev { spec: String },
    Type { kind: LqType },
    Select { dim: LqSelect },
    Fork { mode: LqYesNoOnly },
    Archived { mode: LqYesNoOnly },
    Visibility { mode: LqVisibility },
    Context { name: String },
    Content { leaf: LqLeaf },
}

/// Tri-state for `fork:` / `archived:` per dsl.md §6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqYesNoOnly {
    Yes,
    No,
    Only,
}

impl LqYesNoOnly {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Only => "only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqVisibility {
    Public,
    Private,
    Any,
}

impl LqVisibility {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Any => "any",
        }
    }
}

/// Boolean expression tree. n-ary `All` / `Any` post-normalize collapse.
/// `Empty` is the inert AST emitted when the input contained zero expression
/// atoms (still legal at parser layer per dsl.md §5.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqExpr {
    Empty,
    Leaf(LqLeaf),
    Not(Box<LqExpr>),
    All(Vec<LqExpr>),
    Any(Vec<LqExpr>),
}

/// Directive node per dsl.md §9.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqDirective {
    /// `into:codeql`.
    IntoCodeQl,
    /// `scope:results` (canonical default).
    ScopeResults,
    /// `with:lexical` (canonical default).
    WithLexical,
}

/// Option carriers attached to the canonical query.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LqOptions {
    pub pattern_type: LqPatternType,
    pub case: Option<LqCase>,
    pub count: Option<LqCountBound>,
}

impl LqOptions {
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            pattern_type: LqPatternType::Standard,
            case: None,
            count: None,
        }
    }
}

/// Canonical query: the only shape PRE-NORM emits. Inputs to the hasher.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LqNormalizedQuery {
    pub lq_version: &'static str,
    pub expr: LqExpr,
    pub filters: Vec<LqFilter>,
    pub directives: Vec<LqDirective>,
    pub options: LqOptions,
    pub source_span: LqSpan,
}

impl LqNormalizedQuery {
    #[must_use]
    pub fn empty(source_span: LqSpan) -> Self {
        Self {
            lq_version: LQ_VERSION_TAG,
            expr: LqExpr::Empty,
            filters: Vec::new(),
            directives: Vec::new(),
            options: LqOptions::defaults(),
            source_span,
        }
    }
}

// --- Manual serde impls --------------------------------------------------
//
// Wire shape is deliberately stable and self-describing:
// - Enums use `{ "tag": "...", "payload": ... }` tagged unions
// - Vectors are CBOR/JSON arrays
// - Structs are CBOR/JSON maps with sorted keys at serialize time
//
// Sorted-key ordering is enforced by the canonical CBOR encoder (see
// `hasher::encode_canonical_cbor`), not the serde impls themselves: serde
// only needs a stable, lossless wire shape; canonical encoding is layered on
// top via a separate serializer.

impl serde::Serialize for LqPatternType {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqPatternType {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqPatternType;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqPatternType string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqPatternType, E> {
                match v {
                    "literal" => Ok(LqPatternType::Literal),
                    "keyword" => Ok(LqPatternType::Keyword),
                    "standard" => Ok(LqPatternType::Standard),
                    "regexp" => Ok(LqPatternType::Regexp),
                    "structural" => Ok(LqPatternType::Structural),
                    other => Err(E::unknown_variant(
                        other,
                        &["literal", "keyword", "standard", "regexp", "structural"],
                    )),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqCase {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(match self {
            Self::Sensitive => "yes",
            Self::Insensitive => "no",
        })
    }
}

impl<'de> serde::Deserialize<'de> for LqCase {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqCase;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqCase string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqCase, E> {
                match v {
                    "yes" => Ok(LqCase::Sensitive),
                    "no" => Ok(LqCase::Insensitive),
                    other => Err(E::unknown_variant(other, &["yes", "no"])),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqCountBound {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Bounded(n) => {
                m.serialize_entry("tag", "bounded")?;
                m.serialize_entry("n", n)?;
            }
            Self::All => {
                m.serialize_entry("tag", "all")?;
                m.serialize_entry("n", &0u32)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqCountBound {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqCountBound;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqCountBound map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqCountBound, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut tag: Option<String> = None;
                let mut n: Option<u32> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "n" => n = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| serde::de::Error::missing_field("tag"))?;
                match tag.as_str() {
                    "bounded" => {
                        let n = n.ok_or_else(|| serde::de::Error::missing_field("n"))?;
                        Ok(LqCountBound::Bounded(n))
                    }
                    "all" => Ok(LqCountBound::All),
                    other => Err(serde::de::Error::unknown_variant(
                        other,
                        &["bounded", "all"],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqSelect {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqSelect {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqSelect;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqSelect string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqSelect, E> {
                match v {
                    "repo" => Ok(LqSelect::Repo),
                    "file" => Ok(LqSelect::File),
                    "path" => Ok(LqSelect::Path),
                    "symbol" => Ok(LqSelect::Symbol),
                    "content" => Ok(LqSelect::Content),
                    "content.match" => Ok(LqSelect::ContentMatch),
                    other => Err(E::unknown_variant(
                        other,
                        &["repo", "file", "path", "symbol", "content", "content.match"],
                    )),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqFileScope {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqFileScope {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqFileScope;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqFileScope string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqFileScope, E> {
                match v {
                    "name_and_path" => Ok(LqFileScope::NameAndPath),
                    "path_only" => Ok(LqFileScope::PathOnly),
                    other => Err(E::unknown_variant(other, &["name_and_path", "path_only"])),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqType {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqType {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqType;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqType string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqType, E> {
                match v {
                    "file" => Ok(LqType::File),
                    "path" => Ok(LqType::Path),
                    "symbol" => Ok(LqType::Symbol),
                    "commit" => Ok(LqType::Commit),
                    "diff" => Ok(LqType::Diff),
                    "repo" => Ok(LqType::Repo),
                    other => Err(E::unknown_variant(
                        other,
                        &["file", "path", "symbol", "commit", "diff", "repo"],
                    )),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqYesNoOnly {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqYesNoOnly {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqYesNoOnly;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("yes|no|only string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqYesNoOnly, E> {
                match v {
                    "yes" => Ok(LqYesNoOnly::Yes),
                    "no" => Ok(LqYesNoOnly::No),
                    "only" => Ok(LqYesNoOnly::Only),
                    other => Err(E::unknown_variant(other, &["yes", "no", "only"])),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqVisibility {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqVisibility {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqVisibility;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqVisibility string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqVisibility, E> {
                match v {
                    "public" => Ok(LqVisibility::Public),
                    "private" => Ok(LqVisibility::Private),
                    "any" => Ok(LqVisibility::Any),
                    other => Err(E::unknown_variant(other, &["public", "private", "any"])),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqLeaf {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Keyword(s) => {
                m.serialize_entry("tag", "keyword")?;
                m.serialize_entry("v", s)?;
            }
            Self::Phrase(s) => {
                m.serialize_entry("tag", "phrase")?;
                m.serialize_entry("v", s)?;
            }
            Self::RawString(s) => {
                m.serialize_entry("tag", "raw_string")?;
                m.serialize_entry("v", s)?;
            }
            Self::Regex(s) => {
                m.serialize_entry("tag", "regex")?;
                m.serialize_entry("v", s)?;
            }
            Self::StructuralBlock(s) => {
                m.serialize_entry("tag", "structural_block")?;
                m.serialize_entry("v", s)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqLeaf {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqLeaf;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqLeaf map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqLeaf, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut tag: Option<String> = None;
                let mut v: Option<String> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => v = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| serde::de::Error::missing_field("tag"))?;
                let v = v.ok_or_else(|| serde::de::Error::missing_field("v"))?;
                match tag.as_str() {
                    "keyword" => Ok(LqLeaf::Keyword(v)),
                    "phrase" => Ok(LqLeaf::Phrase(v)),
                    "raw_string" => Ok(LqLeaf::RawString(v)),
                    "regex" => Ok(LqLeaf::Regex(v)),
                    "structural_block" => Ok(LqLeaf::StructuralBlock(v)),
                    other => Err(serde::de::Error::unknown_variant(
                        other,
                        &[
                            "keyword",
                            "phrase",
                            "raw_string",
                            "regex",
                            "structural_block",
                        ],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqExpr {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Empty => {
                m.serialize_entry("tag", "empty")?;
                let empty: [u8; 0] = [];
                m.serialize_entry("v", &empty[..])?;
            }
            Self::Leaf(l) => {
                m.serialize_entry("tag", "leaf")?;
                m.serialize_entry("v", l)?;
            }
            Self::Not(inner) => {
                m.serialize_entry("tag", "not")?;
                m.serialize_entry("v", inner.as_ref())?;
            }
            Self::All(children) => {
                m.serialize_entry("tag", "all")?;
                m.serialize_entry("v", children)?;
            }
            Self::Any(children) => {
                m.serialize_entry("tag", "any")?;
                m.serialize_entry("v", children)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqExpr {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Two-pass: collect map into a buffer, then re-decode by tag. ciborium
        // streams in order so we sniff tag first via raw `ciborium::value::Value`
        // is overkill; instead we use serde_value-like manual buffering.
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqExpr;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqExpr map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqExpr, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut buffered_value: Option<ciborium::value::Value> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => buffered_value = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                let buffered_value = buffered_value.ok_or_else(|| A::Error::missing_field("v"))?;
                match tag.as_str() {
                    "empty" => Ok(LqExpr::Empty),
                    "leaf" => {
                        let leaf: LqLeaf = buffered_value
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("leaf payload: {e}")))?;
                        Ok(LqExpr::Leaf(leaf))
                    }
                    "not" => {
                        let inner: LqExpr = buffered_value
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("not payload: {e}")))?;
                        Ok(LqExpr::Not(Box::new(inner)))
                    }
                    "all" => {
                        let kids: Vec<LqExpr> = buffered_value
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("all payload: {e}")))?;
                        Ok(LqExpr::All(kids))
                    }
                    "any" => {
                        let kids: Vec<LqExpr> = buffered_value
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("any payload: {e}")))?;
                        Ok(LqExpr::Any(kids))
                    }
                    other => Err(A::Error::unknown_variant(
                        other,
                        &["empty", "leaf", "not", "all", "any"],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqFilter {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        // Two-key tag + payload map; payload itself is a nested map of the
        // filter's fields.
        let mut outer = ser.serialize_map(Some(2))?;
        let tag: &str = match self {
            Self::Repo { .. } => "repo",
            Self::File { .. } => "file",
            Self::Lang { .. } => "lang",
            Self::Rev { .. } => "rev",
            Self::Type { .. } => "type",
            Self::Select { .. } => "select",
            Self::Fork { .. } => "fork",
            Self::Archived { .. } => "archived",
            Self::Visibility { .. } => "visibility",
            Self::Context { .. } => "context",
            Self::Content { .. } => "content",
        };
        outer.serialize_entry("tag", tag)?;
        match self {
            Self::Repo { pattern, revs } => {
                let payload = RepoPayload { pattern, revs };
                outer.serialize_entry("v", &payload)?;
            }
            Self::File { pattern, scope } => {
                let payload = FilePayload {
                    pattern,
                    scope: *scope,
                };
                outer.serialize_entry("v", &payload)?;
            }
            Self::Lang { id } => outer.serialize_entry("v", id)?,
            Self::Rev { spec } => outer.serialize_entry("v", spec)?,
            Self::Type { kind } => outer.serialize_entry("v", kind)?,
            Self::Select { dim } => outer.serialize_entry("v", dim)?,
            Self::Fork { mode } => outer.serialize_entry("v", mode)?,
            Self::Archived { mode } => outer.serialize_entry("v", mode)?,
            Self::Visibility { mode } => outer.serialize_entry("v", mode)?,
            Self::Context { name } => outer.serialize_entry("v", name)?,
            Self::Content { leaf } => outer.serialize_entry("v", leaf)?,
        }
        outer.end()
    }
}

struct RepoPayload<'a> {
    pattern: &'a String,
    revs: &'a Vec<String>,
}

impl serde::Serialize for RepoPayload<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("pattern", self.pattern)?;
        m.serialize_entry("revs", self.revs)?;
        m.end()
    }
}

struct FilePayload<'a> {
    pattern: &'a String,
    scope: LqFileScope,
}

impl serde::Serialize for FilePayload<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("pattern", self.pattern)?;
        m.serialize_entry("scope", &self.scope)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqFilter {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqFilter;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqFilter map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqFilter, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut v_value: Option<ciborium::value::Value> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => v_value = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                let v = v_value.ok_or_else(|| A::Error::missing_field("v"))?;
                match tag.as_str() {
                    "repo" => {
                        let payload: RepoOwned = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("repo payload: {e}")))?;
                        Ok(LqFilter::Repo {
                            pattern: payload.pattern,
                            revs: payload.revs,
                        })
                    }
                    "file" => {
                        let payload: FileOwned = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("file payload: {e}")))?;
                        Ok(LqFilter::File {
                            pattern: payload.pattern,
                            scope: payload.scope,
                        })
                    }
                    "lang" => Ok(LqFilter::Lang {
                        id: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("lang: {e}")))?,
                    }),
                    "rev" => Ok(LqFilter::Rev {
                        spec: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("rev: {e}")))?,
                    }),
                    "type" => Ok(LqFilter::Type {
                        kind: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("type: {e}")))?,
                    }),
                    "select" => Ok(LqFilter::Select {
                        dim: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("select: {e}")))?,
                    }),
                    "fork" => Ok(LqFilter::Fork {
                        mode: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("fork: {e}")))?,
                    }),
                    "archived" => Ok(LqFilter::Archived {
                        mode: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("archived: {e}")))?,
                    }),
                    "visibility" => Ok(LqFilter::Visibility {
                        mode: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("visibility: {e}")))?,
                    }),
                    "context" => Ok(LqFilter::Context {
                        name: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("context: {e}")))?,
                    }),
                    "content" => Ok(LqFilter::Content {
                        leaf: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("content: {e}")))?,
                    }),
                    other => Err(A::Error::unknown_variant(
                        other,
                        &[
                            "repo",
                            "file",
                            "lang",
                            "rev",
                            "type",
                            "select",
                            "fork",
                            "archived",
                            "visibility",
                            "context",
                            "content",
                        ],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

struct RepoOwned {
    pattern: String,
    revs: Vec<String>,
}

impl<'de> serde::Deserialize<'de> for RepoOwned {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = RepoOwned;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("repo payload")
            }
            fn visit_map<A>(self, mut map: A) -> Result<RepoOwned, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut pattern: Option<String> = None;
                let mut revs: Option<Vec<String>> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "pattern" => pattern = Some(map.next_value()?),
                        "revs" => revs = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(RepoOwned {
                    pattern: pattern.ok_or_else(|| serde::de::Error::missing_field("pattern"))?,
                    revs: revs.ok_or_else(|| serde::de::Error::missing_field("revs"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

struct FileOwned {
    pattern: String,
    scope: LqFileScope,
}

impl<'de> serde::Deserialize<'de> for FileOwned {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = FileOwned;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("file payload")
            }
            fn visit_map<A>(self, mut map: A) -> Result<FileOwned, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut pattern: Option<String> = None;
                let mut scope: Option<LqFileScope> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "pattern" => pattern = Some(map.next_value()?),
                        "scope" => scope = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(FileOwned {
                    pattern: pattern.ok_or_else(|| serde::de::Error::missing_field("pattern"))?,
                    scope: scope.ok_or_else(|| serde::de::Error::missing_field("scope"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqDirective {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(match self {
            Self::IntoCodeQl => "into:codeql",
            Self::ScopeResults => "scope:results",
            Self::WithLexical => "with:lexical",
        })
    }
}

impl<'de> serde::Deserialize<'de> for LqDirective {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqDirective;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqDirective string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqDirective, E> {
                match v {
                    "into:codeql" => Ok(LqDirective::IntoCodeQl),
                    "scope:results" => Ok(LqDirective::ScopeResults),
                    "with:lexical" => Ok(LqDirective::WithLexical),
                    other => Err(E::unknown_variant(
                        other,
                        &["into:codeql", "scope:results", "with:lexical"],
                    )),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqOptions {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        m.serialize_entry("pattern_type", &self.pattern_type)?;
        match &self.case {
            Some(c) => m.serialize_entry("case", c)?,
            None => m.serialize_entry::<_, Option<LqCase>>("case", &None)?,
        }
        match &self.count {
            Some(c) => m.serialize_entry("count", c)?,
            None => m.serialize_entry::<_, Option<LqCountBound>>("count", &None)?,
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqOptions {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqOptions;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqOptions map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqOptions, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut pattern_type: Option<LqPatternType> = None;
                let mut case: Option<LqCase> = None;
                let mut count: Option<LqCountBound> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "pattern_type" => pattern_type = Some(map.next_value()?),
                        "case" => case = map.next_value()?,
                        "count" => count = map.next_value()?,
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(LqOptions {
                    pattern_type: pattern_type.unwrap_or(LqPatternType::Standard),
                    case,
                    count,
                })
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqNormalizedQuery {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(5))?;
        m.serialize_entry("lq_version", self.lq_version)?;
        m.serialize_entry("expr", &self.expr)?;
        m.serialize_entry("filters", &self.filters)?;
        m.serialize_entry("directives", &self.directives)?;
        m.serialize_entry("options", &self.options)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqNormalizedQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqNormalizedQuery;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqNormalizedQuery map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqNormalizedQuery, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut expr: Option<LqExpr> = None;
                let mut filters: Option<Vec<LqFilter>> = None;
                let mut directives: Option<Vec<LqDirective>> = None;
                let mut options: Option<LqOptions> = None;
                let mut lq_version: Option<String> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "lq_version" => lq_version = Some(map.next_value()?),
                        "expr" => expr = Some(map.next_value()?),
                        "filters" => filters = Some(map.next_value()?),
                        "directives" => directives = Some(map.next_value()?),
                        "options" => options = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                // Discard payload's lq_version string and reuse the static
                // version tag; PRE-NORM owns version authority.
                drop(lq_version);
                Ok(LqNormalizedQuery {
                    lq_version: LQ_VERSION_TAG,
                    expr: expr.ok_or_else(|| serde::de::Error::missing_field("expr"))?,
                    filters: filters.ok_or_else(|| serde::de::Error::missing_field("filters"))?,
                    directives: directives
                        .ok_or_else(|| serde::de::Error::missing_field("directives"))?,
                    options: options.ok_or_else(|| serde::de::Error::missing_field("options"))?,
                    source_span: LqSpan::eof(0),
                })
            }
        }
        de.deserialize_map(V)
    }
}
