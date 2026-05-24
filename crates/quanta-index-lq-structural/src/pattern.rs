//! Structural pattern IR and the §8.1 mini-language parser.
//!
//! [`StructuralPattern`] is the typed result of parsing a `match { … }`
//! body. [`PatternNode`] is the node-level IR: literals, metavariables,
//! and nested groups (anything `{...}`).
//!
//! The mini-language parsed here is the body of `match { ... }` after
//! Sourcegraph alias normalization (LEX-01 / §8.6) has been applied —
//! the parser still accepts the `:[name]` shorthand because callers
//! exercising the structural plane in isolation may pass raw bodies. The
//! `$name` form is preferred and produced as the canonical IR.
//!
//! Caps:
//!
//! - [`crate::types::MAX_STRUCTURAL_NODES`] (256) — total node count.
//! - [`crate::types::MAX_DEPTH`] (16) — nesting depth.
//! - [`crate::types::MAX_METAVARS_PER_PATTERN`] (32) — distinct metavar
//!   names.
//!
//! Any breach surfaces a typed [`crate::errors::StructuralError`].
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;
use std::collections::BTreeSet;

use crate::errors::{LimitDimension, StructuralError, StructuralErrorCode};
use crate::types::{LangId, MAX_DEPTH, MAX_METAVARS_PER_PATTERN, MAX_STRUCTURAL_NODES, MetaVar};

/// Single pattern-IR node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PatternNode {
    /// Verbatim text segment.
    Literal(Box<str>),
    /// Metavariable capture (`$name` / `:[name]`).
    Metavar(MetaVar),
    /// Brace-delimited group; sequence of child nodes in source order.
    Group(Vec<PatternNode>),
}

impl PatternNode {
    /// Count this node plus every descendant.
    pub(crate) fn count_nodes(&self, acc: &mut u32) -> Result<(), StructuralError> {
        *acc = acc.checked_add(1).ok_or_else(|| {
            StructuralError::plan_limit_exceeded(
                LimitDimension::NodeCount,
                "node count overflowed u32",
            )
        })?;
        if *acc > MAX_STRUCTURAL_NODES {
            return Err(StructuralError::plan_limit_exceeded(
                LimitDimension::NodeCount,
                format!("structural pattern exceeds MAX_STRUCTURAL_NODES={MAX_STRUCTURAL_NODES}"),
            ));
        }
        match self {
            Self::Literal(_) | Self::Metavar(_) => Ok(()),
            Self::Group(children) => {
                for c in children {
                    c.count_nodes(acc)?;
                }
                Ok(())
            }
        }
    }

    /// Walk depth.
    pub(crate) fn check_depth(&self, current: u32) -> Result<(), StructuralError> {
        if current > MAX_DEPTH {
            return Err(StructuralError::plan_limit_exceeded(
                LimitDimension::Depth,
                format!("structural pattern depth exceeds MAX_DEPTH={MAX_DEPTH}"),
            ));
        }
        match self {
            Self::Literal(_) | Self::Metavar(_) => Ok(()),
            Self::Group(children) => {
                let next = current.checked_add(1).ok_or_else(|| {
                    StructuralError::plan_limit_exceeded(
                        LimitDimension::Depth,
                        "depth counter overflowed u32",
                    )
                })?;
                for c in children {
                    c.check_depth(next)?;
                }
                Ok(())
            }
        }
    }

    /// Collect every metavariable name reachable from this node.
    pub(crate) fn collect_metavars(&self, into: &mut BTreeSet<MetaVar>) {
        match self {
            Self::Literal(_) => {}
            Self::Metavar(m) => {
                let _inserted: bool = into.insert(m.clone());
            }
            Self::Group(children) => {
                for c in children {
                    c.collect_metavars(into);
                }
            }
        }
    }
}

impl serde::Serialize for PatternNode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Literal(s) => {
                m.serialize_entry("kind", "LITERAL")?;
                m.serialize_entry("value", s.as_ref())?;
            }
            Self::Metavar(v) => {
                m.serialize_entry("kind", "METAVAR")?;
                m.serialize_entry("value", v)?;
            }
            Self::Group(children) => {
                m.serialize_entry("kind", "GROUP")?;
                m.serialize_entry("value", children)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for PatternNode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        enum Held {
            None,
            Literal(String),
            Metavar(MetaVar),
            Group(Vec<PatternNode>),
        }

        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = PatternNode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("PatternNode map (kind, value)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<PatternNode, M::Error> {
                let mut kind: Option<String> = None;
                let mut held: Held = Held::None;
                let mut value_seen = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => {
                            if kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        "value" => {
                            if value_seen {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            value_seen = true;
                            let Some(k) = kind.as_deref() else {
                                return Err(serde::de::Error::custom(
                                    "PatternNode 'kind' must precede 'value'",
                                ));
                            };
                            held = match k {
                                "LITERAL" => Held::Literal(map.next_value()?),
                                "METAVAR" => Held::Metavar(map.next_value()?),
                                "GROUP" => Held::Group(map.next_value()?),
                                other => {
                                    return Err(serde::de::Error::unknown_variant(
                                        other,
                                        &["LITERAL", "METAVAR", "GROUP"],
                                    ));
                                }
                            };
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &["kind", "value"]));
                        }
                    }
                }
                let _kind = kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                match held {
                    Held::None => Err(serde::de::Error::missing_field("value")),
                    Held::Literal(s) => Ok(PatternNode::Literal(s.into_boxed_str())),
                    Held::Metavar(m) => Ok(PatternNode::Metavar(m)),
                    Held::Group(children) => Ok(PatternNode::Group(children)),
                }
            }
        }
        de.deserialize_map(V)
    }
}

/// Parsed structural pattern, anchored to a single [`LangId`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructuralPattern {
    lang: LangId,
    root: PatternNode,
    metavars: BTreeSet<MetaVar>,
}

impl StructuralPattern {
    /// Construct directly without parsing. Useful for fixture / test code
    /// that wants to bypass the surface grammar. Enforces every cap.
    pub fn from_root(lang: LangId, root: PatternNode) -> Result<Self, StructuralError> {
        let mut node_acc: u32 = 0;
        root.count_nodes(&mut node_acc)?;
        root.check_depth(0)?;
        let mut metavars: BTreeSet<MetaVar> = BTreeSet::new();
        root.collect_metavars(&mut metavars);
        let observed_metavars: u32 = check_metavar_count(metavars.len())?;
        if observed_metavars > MAX_METAVARS_PER_PATTERN {
            return Err(StructuralError::plan_limit_exceeded(
                LimitDimension::MetavarCount,
                format!(
                    "structural pattern exceeds MAX_METAVARS_PER_PATTERN={MAX_METAVARS_PER_PATTERN} (observed={observed_metavars})"
                ),
            ));
        }
        Ok(Self {
            lang,
            root,
            metavars,
        })
    }

    /// Borrow the anchor language.
    #[must_use]
    pub const fn lang(&self) -> LangId {
        self.lang
    }

    /// Borrow the pattern's root IR node.
    #[must_use]
    pub const fn root(&self) -> &PatternNode {
        &self.root
    }

    /// Borrow the distinct metavariable set captured by this pattern.
    #[must_use]
    pub const fn metavars(&self) -> &BTreeSet<MetaVar> {
        &self.metavars
    }

    /// `true` if the pattern captures at least one metavariable.
    #[must_use]
    pub fn has_metavars(&self) -> bool {
        !self.metavars.is_empty()
    }
}

impl serde::Serialize for StructuralPattern {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(3))?;
        m.serialize_entry("lang", &self.lang)?;
        m.serialize_entry("root", &self.root)?;
        // Metavars are derived from root but emitted explicitly so the
        // wire shape is self-describing and matches the §4.9 carrier.
        let mv: Vec<&MetaVar> = self.metavars.iter().collect();
        m.serialize_entry("metavars", &mv)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for StructuralPattern {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = StructuralPattern;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("StructuralPattern map (lang, root, metavars)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<StructuralPattern, M::Error> {
                let mut lang: Option<LangId> = None;
                let mut root: Option<PatternNode> = None;
                let mut metavars_in: Option<Vec<MetaVar>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "lang" => {
                            if lang.is_some() {
                                return Err(serde::de::Error::duplicate_field("lang"));
                            }
                            lang = Some(map.next_value()?);
                        }
                        "root" => {
                            if root.is_some() {
                                return Err(serde::de::Error::duplicate_field("root"));
                            }
                            root = Some(map.next_value()?);
                        }
                        "metavars" => {
                            if metavars_in.is_some() {
                                return Err(serde::de::Error::duplicate_field("metavars"));
                            }
                            metavars_in = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["lang", "root", "metavars"],
                            ));
                        }
                    }
                }
                let lang = lang.ok_or_else(|| serde::de::Error::missing_field("lang"))?;
                let root = root.ok_or_else(|| serde::de::Error::missing_field("root"))?;
                // Re-derive metavars from root; cross-check against the
                // wire payload so a tampered wire surfaces a typed error.
                let mut derived: BTreeSet<MetaVar> = BTreeSet::new();
                root.collect_metavars(&mut derived);
                if let Some(declared) = metavars_in {
                    let declared_set: BTreeSet<MetaVar> = declared.into_iter().collect();
                    if declared_set != derived {
                        return Err(serde::de::Error::custom(
                            "StructuralPattern metavars mismatch between declared list and root tree",
                        ));
                    }
                }
                let observed_metavars: u32 = match check_metavar_count(derived.len()) {
                    Ok(v) => v,
                    Err(e) => return Err(serde::de::Error::custom(format!("{e}"))),
                };
                if observed_metavars > MAX_METAVARS_PER_PATTERN {
                    return Err(serde::de::Error::custom(format!(
                        "metavar count {observed_metavars} exceeds cap {MAX_METAVARS_PER_PATTERN}"
                    )));
                }
                let mut node_acc: u32 = 0;
                if let Err(e) = root.count_nodes(&mut node_acc) {
                    return Err(serde::de::Error::custom(format!("{e}")));
                }
                if let Err(e) = root.check_depth(0) {
                    return Err(serde::de::Error::custom(format!("{e}")));
                }
                Ok(StructuralPattern {
                    lang,
                    root,
                    metavars: derived,
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// Parse the body of a `match { ... }` block into a typed pattern IR.
///
/// Accepted surface grammar (subset of `dsl.md` §8.1):
///
/// - literal text — any UTF-8 outside `{`, `}`, and the metavariable
///   start sequence;
/// - `$name` — single-token metavariable;
/// - `:[name]` — Sourcegraph alias for `$name`;
/// - `{ ... }` — nested group; recursive.
///
/// Out-of-context metavar (e.g. empty name `$` / `:[]`) surfaces
/// `STR_INVALID_METAVAR`. Unbalanced braces surface `STR_PARSE_FAIL`.
/// Cap violations surface `PLAN_LIMIT_EXCEEDED`.
///
/// This is the explicit-mini-language path; LEX-01 normalization will
/// have already mapped `:[X] -> $X` for inputs that come from the lexer.
pub fn parse_pattern(raw: &str, lang: LangId) -> Result<StructuralPattern, StructuralError> {
    let mut parser = Parser::new(raw);
    let root = parser.parse_group_body(0)?;
    if parser.pos < parser.input.len() {
        return Err(StructuralError::parse_fail(
            parser.pos,
            "trailing input after structural body",
        ));
    }
    StructuralPattern::from_root(lang, PatternNode::Group(root))
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            input: s.as_bytes(),
            pos: 0,
        }
    }

    /// Bump `pos` by 1 byte, with overflow-safe arithmetic per the
    /// `arithmetic_side_effects = deny` workspace lint.
    fn bump(&mut self) -> Result<(), StructuralError> {
        self.pos = self.pos.checked_add(1).ok_or_else(|| {
            StructuralError::parse_fail(self.pos, "input offset overflowed usize")
        })?;
        Ok(())
    }

    fn parse_group_body(&mut self, depth: u32) -> Result<Vec<PatternNode>, StructuralError> {
        if depth > MAX_DEPTH {
            return Err(StructuralError::plan_limit_exceeded(
                LimitDimension::Depth,
                format!("structural pattern depth exceeds MAX_DEPTH={MAX_DEPTH}"),
            ));
        }
        let mut out: Vec<PatternNode> = Vec::new();
        let mut literal_buf: Vec<u8> = Vec::new();
        loop {
            let Some(&b) = self.input.get(self.pos) else {
                // EOF — flush and return; caller decides if EOF is legal here.
                if !literal_buf.is_empty() {
                    out.push(PatternNode::Literal(bytes_to_box(&literal_buf, self.pos)?));
                    literal_buf.clear();
                }
                return Ok(out);
            };
            match b {
                b'}' => {
                    if !literal_buf.is_empty() {
                        out.push(PatternNode::Literal(bytes_to_box(&literal_buf, self.pos)?));
                        literal_buf.clear();
                    }
                    return Ok(out);
                }
                b'{' => {
                    if !literal_buf.is_empty() {
                        out.push(PatternNode::Literal(bytes_to_box(&literal_buf, self.pos)?));
                        literal_buf.clear();
                    }
                    self.bump()?; // consume '{'
                    let next_depth = depth.checked_add(1).ok_or_else(|| {
                        StructuralError::plan_limit_exceeded(
                            LimitDimension::Depth,
                            "depth counter overflowed u32",
                        )
                    })?;
                    let inner = self.parse_group_body(next_depth)?;
                    // Require closing brace.
                    if self.input.get(self.pos) != Some(&b'}') {
                        return Err(StructuralError::parse_fail(
                            self.pos,
                            "unbalanced '{' — missing closing '}'",
                        ));
                    }
                    self.bump()?; // consume '}'
                    out.push(PatternNode::Group(inner));
                }
                b'$' => {
                    if !literal_buf.is_empty() {
                        out.push(PatternNode::Literal(bytes_to_box(&literal_buf, self.pos)?));
                        literal_buf.clear();
                    }
                    self.bump()?; // consume '$'
                    let mv = self.parse_metavar_ident()?;
                    out.push(PatternNode::Metavar(mv));
                }
                b':' if self.peek_alias() => {
                    if !literal_buf.is_empty() {
                        out.push(PatternNode::Literal(bytes_to_box(&literal_buf, self.pos)?));
                        literal_buf.clear();
                    }
                    self.bump()?; // ':'
                    self.bump()?; // '['
                    let mv = self.parse_metavar_ident_until(b']')?;
                    // consume ']'
                    if self.input.get(self.pos) != Some(&b']') {
                        return Err(StructuralError::parse_fail(
                            self.pos,
                            "metavariable ':[name]' missing closing ']'",
                        ));
                    }
                    self.bump()?;
                    out.push(PatternNode::Metavar(mv));
                }
                _ => {
                    literal_buf.push(b);
                    self.bump()?;
                }
            }
        }
    }

    /// Returns true if the next two bytes are `:[`.
    fn peek_alias(&self) -> bool {
        let Some(n) = self.pos.checked_add(1) else {
            return false;
        };
        self.input.get(n) == Some(&b'[')
    }

    /// Parse `[A-Za-z_][A-Za-z0-9_]*` immediately following `$`.
    fn parse_metavar_ident(&mut self) -> Result<MetaVar, StructuralError> {
        let start = self.pos;
        while let Some(&b) = self.input.get(self.pos) {
            let is_first = self.pos == start;
            let ok = if is_first {
                b.is_ascii_alphabetic() || b == b'_'
            } else {
                b.is_ascii_alphanumeric() || b == b'_'
            };
            if !ok {
                break;
            }
            self.bump()?;
        }
        if self.pos == start {
            return Err(StructuralError::invalid_metavar(""));
        }
        let bytes = self.input.get(start..self.pos).ok_or_else(|| {
            StructuralError::parse_fail(start, "slice bounds out of range parsing metavar")
        })?;
        let name = core::str::from_utf8(bytes).map_err(|e| {
            StructuralError::parse_fail(start, format!("metavar name not utf-8: {e}"))
        })?;
        MetaVar::new(name)
    }

    /// Parse a metavariable name up to (but not including) `terminator`.
    /// Used by the `:[name]` alias form.
    fn parse_metavar_ident_until(&mut self, terminator: u8) -> Result<MetaVar, StructuralError> {
        let start = self.pos;
        while let Some(&b) = self.input.get(self.pos) {
            if b == terminator {
                break;
            }
            self.bump()?;
        }
        if self.pos == start {
            return Err(StructuralError::invalid_metavar(""));
        }
        let bytes = self.input.get(start..self.pos).ok_or_else(|| {
            StructuralError::parse_fail(start, "slice bounds out of range parsing metavar")
        })?;
        let name = core::str::from_utf8(bytes).map_err(|e| {
            StructuralError::parse_fail(start, format!("metavar name not utf-8: {e}"))
        })?;
        MetaVar::new(name)
    }
}

/// Saturating cast of a `usize` metavar-count to `u32`.
///
/// Returns `PLAN_LIMIT_EXCEEDED { METAVAR_COUNT }` if the count cannot
/// fit in `u32` (which already implies it exceeds the cap).
#[expect(
    clippy::option_if_let_else,
    reason = "the clippy-suggested `.map_or_else(...)` form then trips `unnecessary_result_map_or_else` because the success arm is the identity"
)]
fn check_metavar_count(len: usize) -> Result<u32, StructuralError> {
    if let Ok(v) = u32::try_from(len) {
        Ok(v)
    } else {
        Err(StructuralError::plan_limit_exceeded(
            LimitDimension::MetavarCount,
            format!("metavar count {len} exceeds u32 range (cap is {MAX_METAVARS_PER_PATTERN})"),
        ))
    }
}

fn bytes_to_box(buf: &[u8], offset: usize) -> Result<Box<str>, StructuralError> {
    let s = core::str::from_utf8(buf).map_err(|e| {
        StructuralError::parse_fail(offset, format!("literal segment not utf-8: {e}"))
    })?;
    Ok(s.to_owned().into_boxed_str())
}

// -- Convenience: error-code guard used by the registry. -----------------

impl StructuralPattern {
    /// `true` if the pattern's IR is a no-op group with no children.
    /// Used by [`crate::registry::MatcherRegistry`] to decide whether an
    /// empty match-set is legitimate (`true`) or a `STR_LANG_RESOLUTION_EMPTY`
    /// suspect (`false`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match &self.root {
            PatternNode::Group(children) => children.is_empty(),
            PatternNode::Literal(_) | PatternNode::Metavar(_) => false,
        }
    }
}

// Tag this so callers can pattern-match against a sentinel error code
// shape rather than re-parsing the detail string.
impl StructuralError {
    /// `true` if `self.code == StrInvalidMetavar`.
    #[must_use]
    pub fn is_invalid_metavar(&self) -> bool {
        matches!(self.code, StructuralErrorCode::StrInvalidMetavar)
    }
}

#[cfg(test)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "tests pin exact pattern shapes and surface unexpected failures via assert!(false, …)"
)]
mod tests {
    use super::{PatternNode, StructuralPattern, parse_pattern};
    use crate::errors::{LimitDimension, StructuralErrorCode};
    use crate::types::{LangId, MAX_DEPTH, MAX_METAVARS_PER_PATTERN, MetaVar};

    #[test]
    fn parse_pure_literal() {
        let Ok(p) = parse_pattern("hello", LangId::Rust) else {
            assert!(false, "must parse literal");
            return;
        };
        let PatternNode::Group(children) = p.root() else {
            assert!(false, "root must be group");
            return;
        };
        assert_eq!(children.len(), 1);
        let Some(first) = children.first() else {
            assert!(false, "non-empty group must have first child");
            return;
        };
        match first {
            PatternNode::Literal(s) => assert_eq!(s.as_ref(), "hello"),
            other => assert!(false, "expected literal, got {other:?}"),
        }
        assert!(p.metavars().is_empty());
    }

    #[test]
    fn parse_single_metavar_dollar_form() {
        let Ok(p) = parse_pattern("$X", LangId::Rust) else {
            assert!(false, "must parse $X");
            return;
        };
        let PatternNode::Group(children) = p.root() else {
            assert!(false, "root must be group");
            return;
        };
        assert_eq!(children.len(), 1);
        let Some(first) = children.first() else {
            assert!(false, "non-empty group must have first child");
            return;
        };
        match first {
            PatternNode::Metavar(m) => assert_eq!(m.as_str(), "X"),
            other => assert!(false, "expected metavar, got {other:?}"),
        }
        assert_eq!(p.metavars().len(), 1);
    }

    #[test]
    fn parse_single_metavar_alias_form() {
        let Ok(p) = parse_pattern(":[X]", LangId::Rust) else {
            assert!(false, "must parse :[X]");
            return;
        };
        let PatternNode::Group(children) = p.root() else {
            assert!(false, "root must be group");
            return;
        };
        assert_eq!(children.len(), 1);
        let Some(first) = children.first() else {
            assert!(false, "non-empty group must have first child");
            return;
        };
        match first {
            PatternNode::Metavar(m) => assert_eq!(m.as_str(), "X"),
            other => assert!(false, "expected metavar, got {other:?}"),
        }
    }

    #[test]
    fn parse_group_balanced() {
        let Ok(p) = parse_pattern("fn $name(arg) { body }", LangId::Rust) else {
            assert!(false, "must parse balanced");
            return;
        };
        let PatternNode::Group(children) = p.root() else {
            assert!(false, "root must be group");
            return;
        };
        // children: literal "fn ", metavar name, literal "(arg) ", group { ... }
        assert_eq!(children.len(), 4, "got: {children:?}");
        let Some(fourth) = children.get(3) else {
            assert!(false, "expected 4 children");
            return;
        };
        match fourth {
            PatternNode::Group(_) => {}
            other => assert!(false, "expected group, got {other:?}"),
        }
    }

    #[test]
    fn parse_unbalanced_left_brace_fails() {
        match parse_pattern("fn x { y", LangId::Rust) {
            Ok(_) => assert!(false, "must reject unbalanced"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrParseFail),
        }
    }

    #[test]
    fn parse_unbalanced_right_brace_fails() {
        match parse_pattern("fn x } y", LangId::Rust) {
            Ok(_) => assert!(false, "must reject unbalanced right brace"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrParseFail),
        }
    }

    #[test]
    fn parse_invalid_metavar_empty() {
        match parse_pattern("$", LangId::Rust) {
            Ok(_) => assert!(false, "must reject empty metavar"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrInvalidMetavar),
        }
    }

    #[test]
    fn parse_invalid_metavar_alias_empty() {
        match parse_pattern(":[]", LangId::Rust) {
            Ok(_) => assert!(false, "must reject empty alias metavar"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrInvalidMetavar),
        }
    }

    #[test]
    fn parse_invalid_metavar_alias_unbalanced() {
        match parse_pattern(":[X", LangId::Rust) {
            Ok(_) => assert!(false, "must reject unbalanced alias"),
            Err(e) => {
                // Either STR_PARSE_FAIL (missing ']') or STR_INVALID_METAVAR.
                assert!(
                    matches!(
                        e.code,
                        StructuralErrorCode::StrParseFail | StructuralErrorCode::StrInvalidMetavar
                    ),
                    "{e}"
                );
            }
        }
    }

    #[test]
    fn parse_metavar_dedup_in_set() {
        let Ok(p) = parse_pattern("$X and $X again", LangId::Rust) else {
            assert!(false, "must parse");
            return;
        };
        assert_eq!(p.metavars().len(), 1);
    }

    #[test]
    fn depth_cap_enforced() {
        // Build a nested-brace input that exceeds MAX_DEPTH.
        let mut s = String::new();
        // We want depth strictly greater than MAX_DEPTH. Per `parse_group_body`,
        // each `{` increases `depth` by 1; the outer call starts at depth=0.
        // So writing MAX_DEPTH+2 opening braces nests deep enough.
        let opens = MAX_DEPTH.saturating_add(2);
        for _ in 0..opens {
            s.push('{');
        }
        for _ in 0..opens {
            s.push('}');
        }
        match parse_pattern(&s, LangId::Rust) {
            Ok(_) => assert!(false, "depth cap must trip"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::Depth));
            }
        }
    }

    #[test]
    fn node_cap_enforced() {
        // Sequence of distinct metavars exceeds the node cap quickly:
        // 300 `$xN` tokens => >256 metavar nodes (+ trailing literals).
        // But metavar cap is 32, so we'd trip that first. Instead use 300
        // simple braces "{}{}{}..." to count groups.
        let mut s = String::new();
        for _ in 0..300_u32 {
            s.push('{');
            s.push('}');
        }
        match parse_pattern(&s, LangId::Rust) {
            Ok(_) => assert!(false, "node cap must trip"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::NodeCount));
            }
        }
    }

    #[test]
    fn metavar_cap_enforced() {
        // 33 distinct metavar names.
        use core::fmt::Write as _;
        let mut s = String::new();
        let cap_plus_one = MAX_METAVARS_PER_PATTERN.saturating_add(1);
        for i in 0..cap_plus_one {
            if write!(&mut s, "$x{i} ").is_err() {
                assert!(false, "string write should not fail");
                return;
            }
        }
        match parse_pattern(&s, LangId::Rust) {
            Ok(_) => assert!(false, "metavar cap must trip"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::MetavarCount));
            }
        }
    }

    #[test]
    fn invalid_metavar_position_rejected() {
        // ASCII control char immediately after `$` is not a valid ident
        // start; must yield STR_INVALID_METAVAR (empty parsed name).
        match parse_pattern("$ foo", LangId::Rust) {
            Ok(_) => assert!(false, "must reject ' ' after $"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrInvalidMetavar),
        }
    }

    #[test]
    fn from_root_constructs_pattern() {
        let Ok(m) = MetaVar::new("Y") else {
            assert!(false, "metavar");
            return;
        };
        let root = PatternNode::Group(vec![PatternNode::Metavar(m.clone())]);
        let Ok(p) = StructuralPattern::from_root(LangId::Python, root) else {
            assert!(false, "from_root must succeed");
            return;
        };
        assert_eq!(p.lang(), LangId::Python);
        assert!(p.has_metavars());
        assert!(p.metavars().contains(&m));
    }

    #[test]
    fn pattern_serde_roundtrip_via_ciborium() {
        let Ok(p) = parse_pattern("fn $name() { body }", LangId::Rust) else {
            assert!(false, "must parse");
            return;
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&p, &mut buf) {
            assert!(false, "ser: {e}");
        }
        let got: Result<StructuralPattern, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, p),
            Err(e) => assert!(false, "de: {e}"),
        }
    }
}
