// Internal canonical AST.
//
// PRE-NORM owns this shape until PRE-CONTRACT-EXT lands the public `LqQuery`
// carrier in the contract crate; at that point the integration ticket rewires
// `IntoLqQuery for LqNormalizedQuery`. The shape here covers the subset of
// dsl.md §2 that PRE-NORM actually executes: boolean layer, pattern leaves,
// filters, directives, structural-block leaf, and the `LqOptions` knobs
// touched by normalization (case, patterntype, count, timeout).
//
// D18: every serde impl on this module is hand-rolled.

use core::fmt;

use crate::errors::LqSpan;

/// Lq DSL version tag baked into the canonical hash root.
/// Pre-freeze tag per ticket §5 step 39 / dsl.md §11.1; bumps to "1.0" in
/// LEX-01 once the carrier ships.
pub const LQ_VERSION_TAG: &str = "1.0-pre";
const ACCEPTED_LQ_VERSION_TAGS: &[&str] = &["1.0-pre", "1.0"];

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

/// Metavariable name inside a structural pattern (`$X` / `:[X]`).
///
/// Wrapper type kept distinct from `String` so the canonical hash treats it
/// as a tagged leaf rather than free text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LqMetaVar(String);

impl LqMetaVar {
    /// Construct a metavariable wrapper.
    #[must_use]
    pub const fn new(name: String) -> Self {
        Self(name)
    }

    /// Borrow the underlying name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Structural-hole multiplicity per dsl.md §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LqStructuralHoleMultiplicity {
    /// Single-node capture/reference (`$X` / `:[X]`).
    One,
    /// Variadic contiguous sibling span (`$...ARGS` / `:[...ARGS]`).
    Many,
}

/// Structural hole reference used by `where`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LqStructuralHoleRef {
    pub name: LqMetaVar,
    pub multiplicity: LqStructuralHoleMultiplicity,
}

/// RHS operand of a structural `where` constraint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqStructuralConstraintOperand {
    Hole(LqStructuralHoleRef),
    Phrase(String),
    RawString(String),
    Regex(String),
}

/// Conjunction-only structural constraint per dsl.md §8.3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LqStructuralConstraint {
    pub left: LqStructuralHoleRef,
    pub right: LqStructuralConstraintOperand,
}

/// One node inside a structural pattern sequence per dsl.md §8.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqStructuralNode {
    /// Verbatim text segment between holes / groups.
    Literal(Box<str>),
    /// Brace-delimited nested group; sequence of child nodes in order.
    Group(Vec<LqStructuralNode>),
    /// Back-compat single-capture metavariable used by the current live subset.
    MetaVar(LqMetaVar),
    /// Named or anonymous capture hole.
    Hole {
        name: Option<LqMetaVar>,
        multiplicity: LqStructuralHoleMultiplicity,
    },
    /// Anonymous variadic wildcard (`...`).
    WildcardMany,
}

/// One structural expression inside a `match { ... }` block.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqStructuralExpr {
    /// Primary structural pattern sequence.
    Pattern(Vec<LqStructuralNode>),
    /// Post-bind conjunction-only structural constraints.
    Where(Vec<LqStructuralConstraint>),
    /// Ancestor-chain positive context restriction.
    Inside(Box<LqStructuralBlock>),
    /// Ancestor-chain negative context restriction.
    Outside(Box<LqStructuralBlock>),
}

/// Typed structural block: optional language tag plus ordered structural
/// nodes plus the parallel expr view.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LqStructuralBlock {
    /// Optional `lang:<id>` tag captured from the `match` header.
    pub lang: Option<String>,
    /// Ordered structural nodes parsed from the block body.
    pub nodes: Vec<LqStructuralNode>,
    /// Canonical structural expr view. Current v1 surface stores a single
    /// `Pattern(nodes)` entry; richer expr forms remain fail-closed upstream.
    pub exprs: Vec<LqStructuralExpr>,
}

/// One argument to a `<scope>:<head>.<tail>(...)` predicate.
///
/// `Filter` carries the nested filter exactly as parsed (no further
/// validation here; predicate semantics are a planner concern).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqPredicateArg {
    /// Bare keyword / identifier token argument.
    Keyword(String),
    /// `"..."` quoted phrase argument.
    Phrase(String),
    /// `'...'` raw-string argument.
    RawString(String),
    /// Integer numeric literal argument.
    Number(i64),
    /// Nested `name:value` filter shape (e.g. `path:src`).
    Filter { name: String, value: String },
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
    /// `match { ... }` body parsed into a typed structural node tree per
    /// dsl.md §8. Body text excludes the surrounding `match {` / `}`.
    StructuralBlock(LqStructuralBlock),
    /// `<scope>:<head>.<tail>(arg_list)` predicate per dsl.md §3.
    ///
    /// `name` is the canonical dot-joined form (e.g. `repo.has.file`).
    /// Unknown predicate names parse cleanly; semantic rejection is the
    /// planner's job.
    Predicate {
        /// Dot-joined canonical predicate name.
        name: String,
        /// Ordered argument list.
        args: Vec<LqPredicateArg>,
    },
}

/// Filter node per dsl.md §6.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LqFilter {
    Repo { pattern: String, revs: Vec<String> },
    File { pattern: String, scope: LqFileScope },
    Lang { id: String },
    Rev { spec: String },
    Author { pattern: String },
    Committer { pattern: String },
    Message { pattern: String },
    Type { kind: LqType },
    Select { dim: LqSelect },
    Dirty { mode: LqYesNoOnly },
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

/// Carrier for a semantic vector input.
///
/// Either an inline embedding or a server-side handle to a stored vector.
/// Wire shape is a tagged map (manual serde, per D18):
/// `{ "tag": "inline" | "handle", "v": <Vec<f32> | String> }`.
///
/// HNSW handle storage model for [`Self::Handle`]:
///   The `Box<str>` opaque id refers to a server-side stored vector. The
///   storage model (`LanceDB` blob vs in-memory HNSW vs other) is an adapter
///   concern and is intentionally NOT pinned here. The handle MUST be
///   resolvable by the same search-plane generation that the query targets;
///   the generation pin lives on the surrounding request envelope, not on
///   the ref.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticVectorRef {
    /// Inline raw embedding. Dimensionality is a producer-side concern; the
    /// reference adapter accepts dim ≤ 1024.
    Inline(Vec<f32>),
    /// Opaque server-side vector handle. See doc-comment for the storage-model
    /// note.
    Handle(Box<str>),
}

/// Boolean expression tree. n-ary `All` / `Any` post-normalize collapse.
/// `Empty` is the inert AST emitted when the input contained zero expression
/// atoms (still legal at parser layer per dsl.md §5.5).
///
/// `SemanticVector` is a programmatic-only leaf (no DSL string form in v1
/// per dsl.md): producers construct it directly via the AST builder. It is
/// inert through parser / normalizer (pass-through) but participates in the
/// canonical CBOR hash so downstream caches discriminate by vector content.
#[derive(Debug, Clone, PartialEq)]
pub enum LqExpr {
    Empty,
    Leaf(LqLeaf),
    Not(Box<LqExpr>),
    All(Vec<LqExpr>),
    Any(Vec<LqExpr>),
    /// Top-K semantic-vector leaf. `top_k` is the per-leaf cap; the surrounding
    /// request envelope owns the overall cap.
    SemanticVector {
        vector_ref: SemanticVectorRef,
        top_k: u32,
    },
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
    pub timeout_ms: Option<u64>,
}

impl LqOptions {
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            pattern_type: LqPatternType::Standard,
            case: None,
            count: None,
            timeout_ms: None,
        }
    }
}

/// Canonical query: the only shape PRE-NORM emits. Inputs to the hasher.
///
/// `Eq`/`Hash` are intentionally not derived: [`LqExpr::SemanticVector`]
/// carries `Vec<f32>` (no `Eq`/`Hash`). Equality at the query layer is
/// `PartialEq`-based; cache keys go through [`crate::hasher::canonical_hash`].
#[derive(Debug, Clone, PartialEq)]
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

impl serde::Serialize for LqMetaVar {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.0.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqMetaVar {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqMetaVar;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqMetaVar string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqMetaVar, E> {
                Ok(LqMetaVar(v.to_owned()))
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqStructuralHoleMultiplicity {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(match self {
            Self::One => "one",
            Self::Many => "many",
        })
    }
}

impl<'de> serde::Deserialize<'de> for LqStructuralHoleMultiplicity {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqStructuralHoleMultiplicity;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqStructuralHoleMultiplicity string")
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> Result<LqStructuralHoleMultiplicity, E> {
                match v {
                    "one" => Ok(LqStructuralHoleMultiplicity::One),
                    "many" => Ok(LqStructuralHoleMultiplicity::Many),
                    other => Err(E::unknown_variant(other, &["one", "many"])),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for LqStructuralHoleRef {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("multiplicity", &self.multiplicity)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqStructuralHoleRef {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqStructuralHoleRef;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqStructuralHoleRef map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqStructuralHoleRef, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut name: Option<LqMetaVar> = None;
                let mut multiplicity: Option<LqStructuralHoleMultiplicity> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "name" => name = Some(map.next_value()?),
                        "multiplicity" => multiplicity = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(LqStructuralHoleRef {
                    name: name.ok_or_else(|| serde::de::Error::missing_field("name"))?,
                    multiplicity: multiplicity
                        .ok_or_else(|| serde::de::Error::missing_field("multiplicity"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqStructuralConstraintOperand {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Hole(hole) => {
                m.serialize_entry("tag", "hole")?;
                m.serialize_entry("v", hole)?;
            }
            Self::Phrase(text) => {
                m.serialize_entry("tag", "phrase")?;
                m.serialize_entry("v", text)?;
            }
            Self::RawString(text) => {
                m.serialize_entry("tag", "raw_string")?;
                m.serialize_entry("v", text)?;
            }
            Self::Regex(text) => {
                m.serialize_entry("tag", "regex")?;
                m.serialize_entry("v", text)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqStructuralConstraintOperand {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqStructuralConstraintOperand;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqStructuralConstraintOperand map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqStructuralConstraintOperand, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut buffered: Option<ciborium::value::Value> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => buffered = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                let v = buffered.ok_or_else(|| A::Error::missing_field("v"))?;
                match tag.as_str() {
                    "hole" => {
                        let hole: LqStructuralHoleRef = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("hole payload: {e}")))?;
                        Ok(LqStructuralConstraintOperand::Hole(hole))
                    }
                    "phrase" => {
                        let text: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("phrase payload: {e}")))?;
                        Ok(LqStructuralConstraintOperand::Phrase(text))
                    }
                    "raw_string" => {
                        let text: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("raw_string payload: {e}")))?;
                        Ok(LqStructuralConstraintOperand::RawString(text))
                    }
                    "regex" => {
                        let text: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("regex payload: {e}")))?;
                        Ok(LqStructuralConstraintOperand::Regex(text))
                    }
                    other => Err(A::Error::unknown_variant(
                        other,
                        &["hole", "phrase", "raw_string", "regex"],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqStructuralConstraint {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("left", &self.left)?;
        m.serialize_entry("right", &self.right)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqStructuralConstraint {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqStructuralConstraint;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqStructuralConstraint map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqStructuralConstraint, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut left: Option<LqStructuralHoleRef> = None;
                let mut right: Option<LqStructuralConstraintOperand> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "left" => left = Some(map.next_value()?),
                        "right" => right = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(LqStructuralConstraint {
                    left: left.ok_or_else(|| serde::de::Error::missing_field("left"))?,
                    right: right.ok_or_else(|| serde::de::Error::missing_field("right"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqStructuralNode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        match self {
            Self::Literal(s) => {
                m.serialize_entry("tag", "literal")?;
                m.serialize_entry("v", s.as_ref())?;
            }
            Self::Group(children) => {
                m.serialize_entry("tag", "group")?;
                m.serialize_entry("v", children)?;
            }
            Self::MetaVar(mv) => {
                m.serialize_entry("tag", "metavar")?;
                m.serialize_entry("v", mv)?;
            }
            Self::Hole { name, multiplicity } => {
                m.serialize_entry("tag", "hole")?;
                m.serialize_entry("name", name)?;
                m.serialize_entry("multiplicity", multiplicity)?;
            }
            Self::WildcardMany => {
                m.serialize_entry("tag", "wildcard_many")?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqStructuralNode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqStructuralNode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqStructuralNode map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqStructuralNode, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut buffered: Option<ciborium::value::Value> = None;
                let mut hole_name: Option<Option<LqMetaVar>> = None;
                let mut hole_multiplicity: Option<LqStructuralHoleMultiplicity> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => buffered = Some(map.next_value()?),
                        "name" => hole_name = Some(map.next_value()?),
                        "multiplicity" => hole_multiplicity = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                match tag.as_str() {
                    "literal" => {
                        let v = buffered.ok_or_else(|| A::Error::missing_field("v"))?;
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("literal payload: {e}")))?;
                        Ok(LqStructuralNode::Literal(s.into_boxed_str()))
                    }
                    "group" => {
                        let v = buffered.ok_or_else(|| A::Error::missing_field("v"))?;
                        let kids: Vec<LqStructuralNode> = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("group payload: {e}")))?;
                        Ok(LqStructuralNode::Group(kids))
                    }
                    "hole" => Ok(LqStructuralNode::Hole {
                        name: hole_name.ok_or_else(|| A::Error::missing_field("name"))?,
                        multiplicity: hole_multiplicity
                            .ok_or_else(|| A::Error::missing_field("multiplicity"))?,
                    }),
                    "wildcard_many" => Ok(LqStructuralNode::WildcardMany),
                    other => Err(A::Error::unknown_variant(
                        other,
                        &["literal", "group", "hole", "wildcard_many"],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqStructuralExpr {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Pattern(nodes) => {
                m.serialize_entry("tag", "pattern")?;
                m.serialize_entry("v", nodes)?;
            }
            Self::Where(constraints) => {
                m.serialize_entry("tag", "where")?;
                m.serialize_entry("v", constraints)?;
            }
            Self::Inside(block) => {
                m.serialize_entry("tag", "inside")?;
                m.serialize_entry("v", block)?;
            }
            Self::Outside(block) => {
                m.serialize_entry("tag", "outside")?;
                m.serialize_entry("v", block)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqStructuralExpr {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqStructuralExpr;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqStructuralExpr map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqStructuralExpr, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut buffered: Option<ciborium::value::Value> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => buffered = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                let v = buffered.ok_or_else(|| A::Error::missing_field("v"))?;
                match tag.as_str() {
                    "pattern" => {
                        let nodes: Vec<LqStructuralNode> = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("pattern payload: {e}")))?;
                        Ok(LqStructuralExpr::Pattern(nodes))
                    }
                    "where" => {
                        let constraints: Vec<LqStructuralConstraint> = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("where payload: {e}")))?;
                        Ok(LqStructuralExpr::Where(constraints))
                    }
                    "inside" => {
                        let block: LqStructuralBlock = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("inside payload: {e}")))?;
                        Ok(LqStructuralExpr::Inside(Box::new(block)))
                    }
                    "outside" => {
                        let block: LqStructuralBlock = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("outside payload: {e}")))?;
                        Ok(LqStructuralExpr::Outside(Box::new(block)))
                    }
                    other => Err(A::Error::unknown_variant(
                        other,
                        &["pattern", "where", "inside", "outside"],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqStructuralBlock {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        match &self.lang {
            Some(l) => m.serialize_entry("lang", l)?,
            None => m.serialize_entry::<_, Option<String>>("lang", &None)?,
        }
        m.serialize_entry("nodes", &self.nodes)?;
        m.serialize_entry("exprs", &self.exprs)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqStructuralBlock {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqStructuralBlock;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqStructuralBlock map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqStructuralBlock, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut lang: Option<String> = None;
                let mut nodes: Option<Vec<LqStructuralNode>> = None;
                let mut exprs: Option<Vec<LqStructuralExpr>> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "lang" => lang = map.next_value()?,
                        "nodes" => nodes = Some(map.next_value()?),
                        "exprs" => exprs = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let exprs = exprs.unwrap_or_else(|| {
                    nodes.as_ref().map_or_else(Vec::new, |nodes| {
                        vec![LqStructuralExpr::Pattern(nodes.clone())]
                    })
                });
                let nodes = nodes.unwrap_or_else(|| match exprs.as_slice() {
                    [LqStructuralExpr::Pattern(nodes)] => nodes.clone(),
                    _ => Vec::new(),
                });
                Ok(LqStructuralBlock { lang, nodes, exprs })
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for LqPredicateArg {
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
            Self::Number(n) => {
                m.serialize_entry("tag", "number")?;
                m.serialize_entry("v", n)?;
            }
            Self::Filter { name, value } => {
                m.serialize_entry("tag", "filter")?;
                let payload = PredicateFilterPayload { name, value };
                m.serialize_entry("v", &payload)?;
            }
        }
        m.end()
    }
}

struct PredicateFilterPayload<'a> {
    name: &'a String,
    value: &'a String,
}

impl serde::Serialize for PredicateFilterPayload<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("name", self.name)?;
        m.serialize_entry("value", self.value)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqPredicateArg {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = LqPredicateArg;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqPredicateArg map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<LqPredicateArg, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut buffered: Option<ciborium::value::Value> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => buffered = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                let v = buffered.ok_or_else(|| A::Error::missing_field("v"))?;
                match tag.as_str() {
                    "keyword" => {
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("keyword payload: {e}")))?;
                        Ok(LqPredicateArg::Keyword(s))
                    }
                    "phrase" => {
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("phrase payload: {e}")))?;
                        Ok(LqPredicateArg::Phrase(s))
                    }
                    "raw_string" => {
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("raw_string payload: {e}")))?;
                        Ok(LqPredicateArg::RawString(s))
                    }
                    "number" => {
                        let n: i64 = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("number payload: {e}")))?;
                        Ok(LqPredicateArg::Number(n))
                    }
                    "filter" => {
                        let p: PredicateFilterOwned = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("filter payload: {e}")))?;
                        Ok(LqPredicateArg::Filter {
                            name: p.name,
                            value: p.value,
                        })
                    }
                    other => Err(A::Error::unknown_variant(
                        other,
                        &["keyword", "phrase", "raw_string", "number", "filter"],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

struct PredicateFilterOwned {
    name: String,
    value: String,
}

impl<'de> serde::Deserialize<'de> for PredicateFilterOwned {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = PredicateFilterOwned;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("predicate filter payload")
            }
            fn visit_map<A>(self, mut map: A) -> Result<PredicateFilterOwned, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut name: Option<String> = None;
                let mut value: Option<String> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "name" => name = Some(map.next_value()?),
                        "value" => value = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(PredicateFilterOwned {
                    name: name.ok_or_else(|| serde::de::Error::missing_field("name"))?,
                    value: value.ok_or_else(|| serde::de::Error::missing_field("value"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

struct PredicatePayload<'a> {
    name: &'a String,
    args: &'a Vec<LqPredicateArg>,
}

impl serde::Serialize for PredicatePayload<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("name", self.name)?;
        m.serialize_entry("args", self.args)?;
        m.end()
    }
}

struct PredicateOwned {
    name: String,
    args: Vec<LqPredicateArg>,
}

impl<'de> serde::Deserialize<'de> for PredicateOwned {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = PredicateOwned;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("predicate payload")
            }
            fn visit_map<A>(self, mut map: A) -> Result<PredicateOwned, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut name: Option<String> = None;
                let mut args: Option<Vec<LqPredicateArg>> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "name" => name = Some(map.next_value()?),
                        "args" => args = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(PredicateOwned {
                    name: name.ok_or_else(|| serde::de::Error::missing_field("name"))?,
                    args: args.ok_or_else(|| serde::de::Error::missing_field("args"))?,
                })
            }
        }
        de.deserialize_map(V)
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
            Self::StructuralBlock(block) => {
                m.serialize_entry("tag", "structural_block")?;
                m.serialize_entry("v", block)?;
            }
            Self::Predicate { name, args } => {
                m.serialize_entry("tag", "predicate")?;
                let payload = PredicatePayload { name, args };
                m.serialize_entry("v", &payload)?;
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
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut buffered: Option<ciborium::value::Value> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => buffered = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                let v = buffered.ok_or_else(|| A::Error::missing_field("v"))?;
                match tag.as_str() {
                    "keyword" => {
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("keyword payload: {e}")))?;
                        Ok(LqLeaf::Keyword(s))
                    }
                    "phrase" => {
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("phrase payload: {e}")))?;
                        Ok(LqLeaf::Phrase(s))
                    }
                    "raw_string" => {
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("raw_string payload: {e}")))?;
                        Ok(LqLeaf::RawString(s))
                    }
                    "regex" => {
                        let s: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("regex payload: {e}")))?;
                        Ok(LqLeaf::Regex(s))
                    }
                    "structural_block" => {
                        let block: LqStructuralBlock = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("structural payload: {e}")))?;
                        Ok(LqLeaf::StructuralBlock(block))
                    }
                    "predicate" => {
                        let p: PredicateOwned = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("predicate payload: {e}")))?;
                        Ok(LqLeaf::Predicate {
                            name: p.name,
                            args: p.args,
                        })
                    }
                    other => Err(A::Error::unknown_variant(
                        other,
                        &[
                            "keyword",
                            "phrase",
                            "raw_string",
                            "regex",
                            "structural_block",
                            "predicate",
                        ],
                    )),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for SemanticVectorRef {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Inline(vector) => {
                m.serialize_entry("tag", "inline")?;
                m.serialize_entry("v", vector)?;
            }
            Self::Handle(handle) => {
                m.serialize_entry("tag", "handle")?;
                m.serialize_entry("v", handle.as_ref())?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for SemanticVectorRef {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = SemanticVectorRef;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SemanticVectorRef map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<SemanticVectorRef, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de::Error as _;
                let mut tag: Option<String> = None;
                let mut buffered: Option<ciborium::value::Value> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "tag" => tag = Some(map.next_value()?),
                        "v" => buffered = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let tag = tag.ok_or_else(|| A::Error::missing_field("tag"))?;
                let v = buffered.ok_or_else(|| A::Error::missing_field("v"))?;
                match tag.as_str() {
                    "inline" => {
                        let vector: Vec<f32> = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("inline payload: {e}")))?;
                        Ok(SemanticVectorRef::Inline(vector))
                    }
                    "handle" => {
                        let handle: String = v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("handle payload: {e}")))?;
                        Ok(SemanticVectorRef::Handle(handle.into_boxed_str()))
                    }
                    other => Err(A::Error::unknown_variant(other, &["inline", "handle"])),
                }
            }
        }
        de.deserialize_map(V)
    }
}

struct SemanticVectorPayload<'a> {
    vector_ref: &'a SemanticVectorRef,
    top_k: &'a u32,
}

impl serde::Serialize for SemanticVectorPayload<'_> {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("vector_ref", self.vector_ref)?;
        m.serialize_entry("top_k", self.top_k)?;
        m.end()
    }
}

struct SemanticVectorPayloadOwned {
    vector_ref: SemanticVectorRef,
    top_k: u32,
}

impl<'de> serde::Deserialize<'de> for SemanticVectorPayloadOwned {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = SemanticVectorPayloadOwned;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SemanticVectorPayload map")
            }
            fn visit_map<A>(self, mut map: A) -> Result<SemanticVectorPayloadOwned, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut vector_ref: Option<SemanticVectorRef> = None;
                let mut top_k: Option<u32> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "vector_ref" => vector_ref = Some(map.next_value()?),
                        "top_k" => top_k = Some(map.next_value()?),
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(SemanticVectorPayloadOwned {
                    vector_ref: vector_ref
                        .ok_or_else(|| serde::de::Error::missing_field("vector_ref"))?,
                    top_k: top_k.ok_or_else(|| serde::de::Error::missing_field("top_k"))?,
                })
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
            Self::SemanticVector { vector_ref, top_k } => {
                m.serialize_entry("tag", "semantic_vector")?;
                let payload = SemanticVectorPayload { vector_ref, top_k };
                m.serialize_entry("v", &payload)?;
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
                    "semantic_vector" => {
                        let payload: SemanticVectorPayloadOwned =
                            buffered_value.deserialized().map_err(|e| {
                                A::Error::custom(format!("semantic_vector payload: {e}"))
                            })?;
                        Ok(LqExpr::SemanticVector {
                            vector_ref: payload.vector_ref,
                            top_k: payload.top_k,
                        })
                    }
                    other => Err(A::Error::unknown_variant(
                        other,
                        &["empty", "leaf", "not", "all", "any", "semantic_vector"],
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
            Self::Author { .. } => "author",
            Self::Committer { .. } => "committer",
            Self::Message { .. } => "message",
            Self::Type { .. } => "type",
            Self::Select { .. } => "select",
            Self::Dirty { .. } => "dirty",
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
            Self::Author { pattern } => outer.serialize_entry("v", pattern)?,
            Self::Committer { pattern } => outer.serialize_entry("v", pattern)?,
            Self::Message { pattern } => outer.serialize_entry("v", pattern)?,
            Self::Type { kind } => outer.serialize_entry("v", kind)?,
            Self::Select { dim } => outer.serialize_entry("v", dim)?,
            Self::Dirty { mode } => outer.serialize_entry("v", mode)?,
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
                    "author" => Ok(LqFilter::Author {
                        pattern: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("author: {e}")))?,
                    }),
                    "committer" => Ok(LqFilter::Committer {
                        pattern: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("committer: {e}")))?,
                    }),
                    "message" => Ok(LqFilter::Message {
                        pattern: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("message: {e}")))?,
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
                    "dirty" => Ok(LqFilter::Dirty {
                        mode: v
                            .deserialized()
                            .map_err(|e| A::Error::custom(format!("dirty: {e}")))?,
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
                            "author",
                            "committer",
                            "message",
                            "type",
                            "select",
                            "dirty",
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
        let mut m = ser.serialize_map(Some(4))?;
        m.serialize_entry("pattern_type", &self.pattern_type)?;
        match &self.case {
            Some(c) => m.serialize_entry("case", c)?,
            None => m.serialize_entry::<_, Option<LqCase>>("case", &None)?,
        }
        match &self.count {
            Some(c) => m.serialize_entry("count", c)?,
            None => m.serialize_entry::<_, Option<LqCountBound>>("count", &None)?,
        }
        match &self.timeout_ms {
            Some(timeout_ms) => m.serialize_entry("timeout_ms", timeout_ms)?,
            None => m.serialize_entry::<_, Option<u64>>("timeout_ms", &None)?,
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
                let mut timeout_ms: Option<u64> = None;
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "pattern_type" => pattern_type = Some(map.next_value()?),
                        "case" => case = map.next_value()?,
                        "count" => count = map.next_value()?,
                        "timeout_ms" => {
                            timeout_ms = map.next_value::<Option<u64>>()?.or(timeout_ms);
                        }
                        _ => {
                            let _ignored: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(LqOptions {
                    pattern_type: pattern_type.unwrap_or(LqPatternType::Standard),
                    case,
                    count,
                    timeout_ms,
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
                let lq_version =
                    lq_version.ok_or_else(|| serde::de::Error::missing_field("lq_version"))?;
                if !ACCEPTED_LQ_VERSION_TAGS.contains(&lq_version.as_str()) {
                    return Err(serde::de::Error::unknown_variant(
                        lq_version.as_str(),
                        ACCEPTED_LQ_VERSION_TAGS,
                    ));
                }
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
