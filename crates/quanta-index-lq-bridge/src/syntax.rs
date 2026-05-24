//! Sourcegraph-syntax v1 lexer + AST.
//!
//! Scope lock: this is a **v1 parser**. The full Sourcegraph grammar is
//! large; we only accept the documented subset listed in [BRIDGE-01 §
//! 6.1](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md).
//! Unknown filter names are rejected with `BRIDGE_UNSUPPORTED_FILTER`
//! at parse time; refused-but-known filters surface during
//! [`crate::translator::translate`].
//!
//! Grammar accepted by [`parse_sourcegraph`]:
//!
//! ```text
//! query   := term (WS term)*
//! term    := group | NOT term | filter | pattern
//! group   := '(' query ')'
//! filter  := ident ':' value
//! ident   := [A-Za-z][A-Za-z0-9_]*
//! value   := non-whitespace, non-')' run (quoted runs unsupported v1)
//! pattern := one of:
//!              '"' ... '"'   — phrase
//!              non-keyword run of non-whitespace, non-')' bytes
//! ```
//!
//! `AND` / `OR` / `NOT` are reserved keywords (case-sensitive). Implicit
//! conjunction between adjacent terms is folded into [`SgQuery::And`].
//! Two adjacent terms with no infix operator are AND-joined (matching
//! Sourcegraph's documented default).
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::{BridgeError, BridgeErrorCode};

/// Closed set of v1-recognized Sourcegraph filter names.
///
/// Order matches [BRIDGE-01 § 6.1](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md);
/// every filter here either reaches `translator::translate` as
/// adopted/normalized or is refused there.
///
/// Unknown filter names parse-fail with `BRIDGE_UNSUPPORTED_FILTER`.
/// This v1 list is deliberately tight — adding a new filter requires
/// updating both this enum and the translator decision table.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SgFilter {
    Repo(Box<str>),
    File(Box<str>),
    Lang(Box<str>),
    Type(Box<str>),
    Case(Box<str>),
    Select(Box<str>),
    Count(Box<str>),
    Patterntype(Box<str>),
    Fork(Box<str>),
    Archived(Box<str>),
    Content(Box<str>),
    Visibility(Box<str>),
    Context(Box<str>),
    Index(Box<str>),
}

impl SgFilter {
    /// Filter-name keyword (e.g. `"repo"`, `"file"`).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Repo(_) => "repo",
            Self::File(_) => "file",
            Self::Lang(_) => "lang",
            Self::Type(_) => "type",
            Self::Case(_) => "case",
            Self::Select(_) => "select",
            Self::Count(_) => "count",
            Self::Patterntype(_) => "patterntype",
            Self::Fork(_) => "fork",
            Self::Archived(_) => "archived",
            Self::Content(_) => "content",
            Self::Visibility(_) => "visibility",
            Self::Context(_) => "context",
            Self::Index(_) => "index",
        }
    }

    /// Filter value (the right-hand side of `name:value`).
    #[must_use]
    pub fn value(&self) -> &str {
        match self {
            Self::Repo(v)
            | Self::File(v)
            | Self::Lang(v)
            | Self::Type(v)
            | Self::Case(v)
            | Self::Select(v)
            | Self::Count(v)
            | Self::Patterntype(v)
            | Self::Fork(v)
            | Self::Archived(v)
            | Self::Content(v)
            | Self::Visibility(v)
            | Self::Context(v)
            | Self::Index(v) => v,
        }
    }

    /// Construct an `SgFilter` by name; returns `None` for an unknown
    /// filter. Used by the parser to fail-closed on unregistered
    /// filters.
    #[must_use]
    pub fn from_name_value(name: &str, value: &str) -> Option<Self> {
        let v = Box::<str>::from(value);
        let f = match name {
            "repo" => Self::Repo(v),
            "file" => Self::File(v),
            "lang" => Self::Lang(v),
            "type" => Self::Type(v),
            "case" => Self::Case(v),
            "select" => Self::Select(v),
            "count" => Self::Count(v),
            "patterntype" => Self::Patterntype(v),
            "fork" => Self::Fork(v),
            "archived" => Self::Archived(v),
            "content" => Self::Content(v),
            "visibility" => Self::Visibility(v),
            "context" => Self::Context(v),
            "index" => Self::Index(v),
            _ => return None,
        };
        Some(f)
    }
}

impl fmt::Display for SgFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name(), self.value())
    }
}

impl serde::Serialize for SgFilter {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("name", self.name())?;
        m.serialize_entry("value", self.value())?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for SgFilter {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = SgFilter;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SgFilter map (name, value)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<SgFilter, M::Error> {
                let mut name: Option<String> = None;
                let mut value: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => {
                            if name.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        "value" => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &["name", "value"]));
                        }
                    }
                }
                let name = name.ok_or_else(|| serde::de::Error::missing_field("name"))?;
                let value = value.ok_or_else(|| serde::de::Error::missing_field("value"))?;
                SgFilter::from_name_value(&name, &value).ok_or_else(|| {
                    serde::de::Error::custom(format!("unknown SgFilter name `{name}`"))
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// v1 Sourcegraph query AST.
///
/// [`Self::Filtered`] carries one or more leading filters followed by a
/// body subquery — this is the canonical filter-prefixed shape the
/// translator lowers most easily. Pure-pattern queries, `AND`/`OR`/`NOT`
/// trees, and bare phrases all collapse onto the other variants.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SgQuery {
    Pattern(Box<str>),
    And(Vec<SgQuery>),
    Or(Vec<SgQuery>),
    Not(Box<SgQuery>),
    Filtered {
        filters: Vec<SgFilter>,
        body: Box<SgQuery>,
    },
}

impl SgQuery {
    /// Tag string used by the hand-rolled serde impl.
    const fn tag(&self) -> &'static str {
        match self {
            Self::Pattern(_) => "pattern",
            Self::And(_) => "and",
            Self::Or(_) => "or",
            Self::Not(_) => "not",
            Self::Filtered { .. } => "filtered",
        }
    }
}

impl serde::Serialize for SgQuery {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let n = match self {
            Self::Pattern(_) | Self::And(_) | Self::Or(_) | Self::Not(_) => 2,
            Self::Filtered { .. } => 3,
        };
        let mut m = ser.serialize_map(Some(n))?;
        m.serialize_entry("tag", self.tag())?;
        match self {
            Self::Pattern(p) => {
                m.serialize_entry("value", p.as_ref())?;
            }
            Self::And(xs) | Self::Or(xs) => {
                m.serialize_entry("value", xs)?;
            }
            Self::Not(inner) => {
                m.serialize_entry("value", inner.as_ref())?;
            }
            Self::Filtered { filters, body } => {
                m.serialize_entry("filters", filters)?;
                m.serialize_entry("body", body.as_ref())?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for SgQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Use a permissive untagged-ish decoder: read all entries into
        // a temp map, then dispatch on `tag`. `tag` MUST come before
        // `value` in the encoded form.
        use serde::de::Error as _;
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = SgQuery;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SgQuery tagged map (tag + payload)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<SgQuery, M::Error> {
                let mut tag: Option<String> = None;
                let mut s_value: Option<String> = None;
                let mut l_value: Option<Vec<SgQuery>> = None;
                let mut sub_value: Option<Box<SgQuery>> = None;
                let mut filters: Option<Vec<SgFilter>> = None;
                let mut body: Option<Box<SgQuery>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "tag" => {
                            if tag.is_some() {
                                return Err(M::Error::duplicate_field("tag"));
                            }
                            tag = Some(map.next_value()?);
                        }
                        "value" => {
                            // The value field is polymorphic; the tag
                            // disambiguates. We peek the tag first if
                            // already seen; otherwise tentatively
                            // decode as the most common shape (string)
                            // and retry as list / subquery on failure.
                            // To keep the impl bounded, require `tag`
                            // to come before `value` in the encoding.
                            let t = tag.as_deref().ok_or_else(|| {
                                M::Error::custom("`tag` must precede `value` in SgQuery encoding")
                            })?;
                            match t {
                                "pattern" => {
                                    if s_value.is_some() {
                                        return Err(M::Error::duplicate_field("value"));
                                    }
                                    s_value = Some(map.next_value()?);
                                }
                                "and" | "or" => {
                                    if l_value.is_some() {
                                        return Err(M::Error::duplicate_field("value"));
                                    }
                                    l_value = Some(map.next_value()?);
                                }
                                "not" => {
                                    if sub_value.is_some() {
                                        return Err(M::Error::duplicate_field("value"));
                                    }
                                    sub_value = Some(map.next_value()?);
                                }
                                "filtered" => {
                                    return Err(M::Error::custom(
                                        "filtered SgQuery must use `filters` + `body` keys, not `value`",
                                    ));
                                }
                                other => {
                                    return Err(M::Error::custom(format!(
                                        "unknown SgQuery tag `{other}`"
                                    )));
                                }
                            }
                        }
                        "filters" => {
                            if filters.is_some() {
                                return Err(M::Error::duplicate_field("filters"));
                            }
                            filters = Some(map.next_value()?);
                        }
                        "body" => {
                            if body.is_some() {
                                return Err(M::Error::duplicate_field("body"));
                            }
                            body = Some(map.next_value()?);
                        }
                        other => {
                            return Err(M::Error::unknown_field(
                                other,
                                &["tag", "value", "filters", "body"],
                            ));
                        }
                    }
                }
                let t = tag.ok_or_else(|| M::Error::missing_field("tag"))?;
                match t.as_str() {
                    "pattern" => {
                        let s = s_value.ok_or_else(|| M::Error::missing_field("value"))?;
                        Ok(SgQuery::Pattern(s.into_boxed_str()))
                    }
                    "and" => {
                        let xs = l_value.ok_or_else(|| M::Error::missing_field("value"))?;
                        Ok(SgQuery::And(xs))
                    }
                    "or" => {
                        let xs = l_value.ok_or_else(|| M::Error::missing_field("value"))?;
                        Ok(SgQuery::Or(xs))
                    }
                    "not" => {
                        let inner = sub_value.ok_or_else(|| M::Error::missing_field("value"))?;
                        Ok(SgQuery::Not(inner))
                    }
                    "filtered" => {
                        let filters = filters.ok_or_else(|| M::Error::missing_field("filters"))?;
                        let body = body.ok_or_else(|| M::Error::missing_field("body"))?;
                        Ok(SgQuery::Filtered { filters, body })
                    }
                    other => Err(M::Error::custom(format!("unknown SgQuery tag `{other}`"))),
                }
            }
        }
        de.deserialize_map(V)
    }
}

/// Parse a v1 Sourcegraph query string into [`SgQuery`].
///
/// Returns `BRIDGE_UNSUPPORTED_FILTER` for unrecognized filter names
/// and `BRIDGE_UNSUPPORTED_DIRECTIVE` for empty input and other
/// directive-shaped refusals.
pub fn parse_sourcegraph(raw: &str) -> Result<SgQuery, BridgeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::unsupported_directive(
            "",
            "empty Sourcegraph query",
        ));
    }
    let mut p = Parser::new(trimmed);
    let q = p.parse_or()?;
    p.skip_ws();
    if !p.is_eof() {
        return Err(BridgeError::new(
            BridgeErrorCode::BridgeTranslateFail,
            None,
            format!("unexpected trailing input at offset {}", p.pos),
        ));
    }
    Ok(q)
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            src: s.as_bytes(),
            pos: 0,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' {
                self.pos = self.pos.saturating_add(1);
            } else {
                break;
            }
        }
    }

    /// Match a keyword (case-sensitive) followed by whitespace, `(`,
    /// or EOF. Advances `pos` on success; restores it on failure.
    fn match_keyword(&mut self, kw: &str) -> bool {
        let kw_bytes = kw.as_bytes();
        let end = self.pos.saturating_add(kw_bytes.len());
        if end > self.src.len() {
            return false;
        }
        let Some(slice) = self.src.get(self.pos..end) else {
            return false;
        };
        if slice != kw_bytes {
            return false;
        }
        // Must be followed by whitespace, EOF, or a paren.
        let next = self.src.get(end).copied();
        let ok = next.is_none_or(|b| b == b' ' || b == b'\t' || b == b'(' || b == b')');
        if ok {
            self.pos = end;
        }
        ok
    }

    fn parse_or(&mut self) -> Result<SgQuery, BridgeError> {
        let mut terms: Vec<SgQuery> = Vec::new();
        let first = self.parse_and()?;
        terms.push(first);
        loop {
            self.skip_ws();
            let save = self.pos;
            if self.match_keyword("OR") {
                let rhs = self.parse_and()?;
                terms.push(rhs);
            } else {
                self.pos = save;
                break;
            }
        }
        if terms.len() == 1 {
            terms
                .into_iter()
                .next()
                .ok_or_else(|| BridgeError::translate_fail("internal: empty OR vec after parse"))
        } else {
            Ok(SgQuery::Or(terms))
        }
    }

    fn parse_and(&mut self) -> Result<SgQuery, BridgeError> {
        let mut terms: Vec<SgQuery> = Vec::new();
        let first = self.parse_term()?;
        terms.push(first);
        loop {
            self.skip_ws();
            if self.is_eof() {
                break;
            }
            // Stop at closing paren or OR; both belong to higher
            // levels.
            if self.peek() == Some(b')') {
                break;
            }
            let save = self.pos;
            if self.match_keyword("OR") {
                self.pos = save;
                break;
            }
            // Explicit AND is just a separator; consume it if present.
            let _consumed_and: bool = self.match_keyword("AND");
            self.skip_ws();
            if self.is_eof() {
                break;
            }
            if self.peek() == Some(b')') {
                break;
            }
            let next = self.parse_term()?;
            terms.push(next);
        }
        if terms.len() == 1 {
            terms
                .into_iter()
                .next()
                .ok_or_else(|| BridgeError::translate_fail("internal: empty AND vec after parse"))
        } else {
            // Fold adjacent leading filters into Filtered{filters, body}.
            Ok(collapse_filters(SgQuery::And(terms)))
        }
    }

    fn parse_term(&mut self) -> Result<SgQuery, BridgeError> {
        self.skip_ws();
        if self.is_eof() {
            return Err(BridgeError::new(
                BridgeErrorCode::BridgeTranslateFail,
                None,
                format!("unexpected EOF at offset {}", self.pos),
            ));
        }
        if self.match_keyword("NOT") {
            self.skip_ws();
            let inner = self.parse_term()?;
            return Ok(SgQuery::Not(Box::new(inner)));
        }
        if self.peek() == Some(b'(') {
            self.pos = self.pos.saturating_add(1);
            self.skip_ws();
            let inner = self.parse_or()?;
            self.skip_ws();
            if self.peek() != Some(b')') {
                return Err(BridgeError::new(
                    BridgeErrorCode::BridgeTranslateFail,
                    None,
                    format!("unmatched `(` opened earlier; pos={}", self.pos),
                ));
            }
            self.pos = self.pos.saturating_add(1);
            return Ok(inner);
        }
        // Phrase: "..."
        if self.peek() == Some(b'"') {
            return self.parse_phrase();
        }
        // Filter or pattern: read a non-whitespace, non-')' run.
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' || b == b')' {
                break;
            }
            self.pos = self.pos.saturating_add(1);
        }
        let raw = self
            .src
            .get(start..self.pos)
            .ok_or_else(|| BridgeError::translate_fail("internal: term-read slice OOB"))?;
        let run = core::str::from_utf8(raw).map_err(|err| {
            BridgeError::new(
                BridgeErrorCode::BridgeTranslateFail,
                None,
                format!("non-utf8 in query at offset {start}: {err}"),
            )
        })?;
        // Detect `name:value` filter shape.
        if let Some(colon_idx) = filter_colon_index(run) {
            let name = run
                .get(..colon_idx)
                .ok_or_else(|| BridgeError::translate_fail("internal: filter name slice OOB"))?;
            let value_start = colon_idx
                .checked_add(1)
                .ok_or_else(|| BridgeError::translate_fail("internal: filter colon overflow"))?;
            let value = run
                .get(value_start..)
                .ok_or_else(|| BridgeError::translate_fail("internal: filter value slice OOB"))?;
            return SgFilter::from_name_value(name, value).map_or_else(
                || {
                    Err(BridgeError::unsupported_filter(
                        name,
                        format!("unknown Sourcegraph filter `{name}:{value}`"),
                    ))
                },
                |f| {
                    Ok(SgQuery::Filtered {
                        filters: vec![f],
                        body: Box::new(SgQuery::Pattern(Box::<str>::from(""))),
                    })
                },
            );
        }
        // Bare pattern. Reject Sourcegraph short-aliases that we don't
        // ship; this is the documented refused set from
        // [BRIDGE-01 § 6.1].
        Ok(SgQuery::Pattern(Box::<str>::from(run)))
    }

    fn parse_phrase(&mut self) -> Result<SgQuery, BridgeError> {
        // Already at `"`. Consume.
        self.pos = self.pos.saturating_add(1);
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == b'"' {
                let slice = self
                    .src
                    .get(start..self.pos)
                    .ok_or_else(|| BridgeError::translate_fail("internal: phrase slice OOB"))?;
                let s = core::str::from_utf8(slice).map_err(|err| {
                    BridgeError::new(
                        BridgeErrorCode::BridgeTranslateFail,
                        None,
                        format!("non-utf8 in phrase at offset {start}: {err}"),
                    )
                })?;
                self.pos = self.pos.saturating_add(1);
                return Ok(SgQuery::Pattern(Box::<str>::from(s)));
            }
            self.pos = self.pos.saturating_add(1);
        }
        Err(BridgeError::new(
            BridgeErrorCode::BridgeTranslateFail,
            None,
            format!("unterminated phrase starting at offset {start}"),
        ))
    }
}

/// Position of the first `:` that delimits a filter name from its value.
///
/// Returns `Some(idx)` only when the prefix before `:` is a
/// syntactically legal filter identifier (`[A-Za-z][A-Za-z0-9_]*`).
/// `None` otherwise so the run is treated as a bare pattern.
fn filter_colon_index(run: &str) -> Option<usize> {
    let bytes = run.as_bytes();
    // Find the first colon.
    let colon = bytes.iter().position(|b| *b == b':')?;
    if colon == 0 {
        return None;
    }
    let head = bytes.get(..colon)?;
    let first = *head.first()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    for b in head.iter().skip(1) {
        if !(b.is_ascii_alphanumeric() || *b == b'_') {
            return None;
        }
    }
    Some(colon)
}

/// Fold a flat `And(vec![Filtered{f1, body=""}, Filtered{f2,
/// body=""}, ..., Pattern(p)])` shape into a single
/// `Filtered{filters: [f1, f2, ...], body: Pattern(p)}`. Non-uniform
/// shapes are left as-is.
fn collapse_filters(q: SgQuery) -> SgQuery {
    let SgQuery::And(terms) = q else {
        return q;
    };
    let mut filters: Vec<SgFilter> = Vec::new();
    let mut body_terms: Vec<SgQuery> = Vec::new();
    for t in terms {
        if let SgQuery::Filtered { filters: fs, body } = t {
            for f in fs {
                filters.push(f);
            }
            // If the body is the empty placeholder from
            // `parse_term`, skip it; otherwise keep it.
            if is_empty_pattern_placeholder(&body) {
                continue;
            }
            body_terms.push(*body);
        } else {
            body_terms.push(t);
        }
    }
    let body = if body_terms.len() == 1 {
        body_terms
            .into_iter()
            .next()
            .unwrap_or_else(|| SgQuery::Pattern(Box::<str>::from("")))
    } else if body_terms.is_empty() {
        SgQuery::Pattern(Box::<str>::from(""))
    } else {
        SgQuery::And(body_terms)
    };
    if filters.is_empty() {
        body
    } else {
        SgQuery::Filtered {
            filters,
            body: Box::new(body),
        }
    }
}

fn is_empty_pattern_placeholder(q: &SgQuery) -> bool {
    matches!(q, SgQuery::Pattern(p) if p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{SgFilter, SgQuery, parse_sourcegraph};
    use crate::errors::BridgeErrorCode;

    fn unwrap_ok(q: Result<SgQuery, crate::errors::BridgeError>) -> SgQuery {
        match q {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "parse failed: {e}");
                SgQuery::Pattern(Box::<str>::from(""))
            }
        }
    }

    #[test]
    fn parses_bare_pattern() {
        let q = unwrap_ok(parse_sourcegraph("foo"));
        let SgQuery::Pattern(p) = q else {
            assert!(false, "expected Pattern");
            return;
        };
        assert_eq!(&*p, "foo");
    }

    #[test]
    fn parses_phrase() {
        let q = unwrap_ok(parse_sourcegraph("\"hello world\""));
        let SgQuery::Pattern(p) = q else {
            assert!(false, "expected Pattern(phrase)");
            return;
        };
        assert_eq!(&*p, "hello world");
    }

    #[test]
    fn parses_single_filter() {
        let q = unwrap_ok(parse_sourcegraph("repo:acme/foo"));
        let SgQuery::Filtered { filters, body } = q else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 1);
        let Some(SgFilter::Repo(v)) = filters.first() else {
            assert!(false, "expected Repo");
            return;
        };
        assert_eq!(&**v, "acme/foo");
        let SgQuery::Pattern(p) = *body else {
            assert!(false, "expected empty body");
            return;
        };
        assert_eq!(&*p, "");
    }

    #[test]
    fn parses_filter_plus_pattern() {
        let q = unwrap_ok(parse_sourcegraph("repo:acme/foo bar"));
        let SgQuery::Filtered { filters, body } = q else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 1);
        let SgQuery::Pattern(p) = *body else {
            assert!(false, "expected Pattern body");
            return;
        };
        assert_eq!(&*p, "bar");
    }

    #[test]
    fn parses_multi_filter() {
        let q = unwrap_ok(parse_sourcegraph("repo:a file:b lang:rust foo"));
        let SgQuery::Filtered { filters, body } = q else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 3);
        let SgQuery::Pattern(p) = *body else {
            assert!(false, "expected Pattern body");
            return;
        };
        assert_eq!(&*p, "foo");
    }

    #[test]
    fn parses_explicit_and() {
        let q = unwrap_ok(parse_sourcegraph("foo AND bar"));
        let SgQuery::And(xs) = q else {
            assert!(false, "expected And");
            return;
        };
        assert_eq!(xs.len(), 2);
    }

    #[test]
    fn parses_or() {
        let q = unwrap_ok(parse_sourcegraph("foo OR bar"));
        let SgQuery::Or(xs) = q else {
            assert!(false, "expected Or");
            return;
        };
        assert_eq!(xs.len(), 2);
    }

    #[test]
    fn parses_not() {
        let q = unwrap_ok(parse_sourcegraph("NOT foo"));
        let SgQuery::Not(inner) = q else {
            assert!(false, "expected Not");
            return;
        };
        let SgQuery::Pattern(p) = *inner else {
            assert!(false, "expected Pattern inside Not");
            return;
        };
        assert_eq!(&*p, "foo");
    }

    #[test]
    fn parses_parentheses() {
        let q = unwrap_ok(parse_sourcegraph("(foo OR bar) AND baz"));
        let SgQuery::And(xs) = q else {
            assert!(false, "expected And");
            return;
        };
        assert_eq!(xs.len(), 2);
    }

    #[test]
    fn rejects_empty_query() {
        match parse_sourcegraph("") {
            Ok(_) => assert!(false, "empty must reject"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn rejects_whitespace_only() {
        match parse_sourcegraph("   \t  ") {
            Ok(_) => assert!(false, "whitespace-only must reject"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn rejects_unknown_filter() {
        match parse_sourcegraph("colorscheme:dark") {
            Ok(_) => assert!(false, "unknown filter must reject"),
            Err(e) => {
                assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedFilter);
                match e.source_construct.as_deref() {
                    Some(s) => assert_eq!(s, "colorscheme"),
                    None => assert!(false, "must carry source_construct"),
                }
            }
        }
    }

    #[test]
    fn rejects_short_alias_r_filter() {
        // `r:` is a Sourcegraph UI alias for `repo:`. We refuse it
        // because we lock the canonical filter set.
        match parse_sourcegraph("r:foo bar") {
            Ok(_) => assert!(false, "short alias must reject"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedFilter),
        }
    }

    #[test]
    fn rejects_unmatched_paren() {
        match parse_sourcegraph("(foo") {
            Ok(_) => assert!(false, "unmatched paren must reject"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeTranslateFail),
        }
    }

    #[test]
    fn rejects_trailing_close_paren() {
        match parse_sourcegraph("foo)") {
            Ok(_) => assert!(false, "trailing `)` must reject"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeTranslateFail),
        }
    }

    #[test]
    fn rejects_unterminated_phrase() {
        match parse_sourcegraph("\"hello") {
            Ok(_) => assert!(false, "unterminated phrase must reject"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeTranslateFail),
        }
    }

    #[test]
    fn filter_with_slash_value() {
        let q = unwrap_ok(parse_sourcegraph("repo:^github.com/acme/.*$"));
        let SgQuery::Filtered { filters, .. } = q else {
            assert!(false, "expected Filtered");
            return;
        };
        let Some(SgFilter::Repo(v)) = filters.first() else {
            assert!(false, "expected Repo");
            return;
        };
        assert_eq!(&**v, "^github.com/acme/.*$");
    }

    #[test]
    fn pattern_with_colon_is_pattern_not_filter() {
        // Colon-bearing pattern with a non-identifier head reads as
        // a bare pattern (e.g. `:abc` has no name part).
        let q = unwrap_ok(parse_sourcegraph(":abc"));
        let SgQuery::Pattern(p) = q else {
            assert!(false, "expected Pattern");
            return;
        };
        assert_eq!(&*p, ":abc");
    }

    #[test]
    fn sgfilter_serde_roundtrip_via_ciborium() {
        let f = SgFilter::Lang(Box::<str>::from("rust"));
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&f, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<SgFilter, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, f),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn sgquery_serde_roundtrip_pattern() {
        let q = SgQuery::Pattern(Box::<str>::from("foo"));
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&q, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<SgQuery, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, q),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn sgquery_serde_roundtrip_filtered() {
        let q = SgQuery::Filtered {
            filters: vec![
                SgFilter::Repo(Box::<str>::from("acme/foo")),
                SgFilter::Lang(Box::<str>::from("rust")),
            ],
            body: Box::new(SgQuery::Pattern(Box::<str>::from("bar"))),
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&q, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<SgQuery, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, q),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
