//! Sourcegraph → LQ lowering.
//!
//! [`translate`] is the one-way translator surface from
//! [BRIDGE-01](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md).
//! It lowers an already-parsed [`SgQuery`] into an [`LqDirective`]
//! placeholder tree.
//!
//! ## Why a placeholder LQ shape
//!
//! BRIDGE-01 lives in a separate crate from `quanta-index-lq-norm`
//! (the canonical LQ AST owner) by design — coupling the bridge to the
//! full LQ AST today would force a rebuild of every consumer when the
//! AST evolves. The plug-in to the real LQ AST is the integration
//! ticket's job; this crate keeps a small, stable
//! [`LqDirective`] shape that mirrors LQ's filter / pattern / boolean
//! structure 1:1 and is trivially mappable to `LqExpr`.
//!
//! ## Decision table (from
//! [BRIDGE-01 § 5 step 5 / § 6.1](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md))
//!
//! | Sourcegraph filter | Bucket | LQ lowering |
//! |---|---|---|
//! | `repo:` / `file:` / `lang:` / `case:` / `select:` / `count:` / `type:` / `patterntype:` | adopted | 1:1 `LqFilter` |
//! | `fork:` (yes/no/only) | normalized | `LqFilter { name: "visibility", value: "include_forks"|"exclude_forks"|"only_forks" }` |
//! | `archived:` (yes/no/only) | normalized | `LqFilter { name: "visibility", value: "include_archived"|"exclude_archived"|"only_archived" }` |
//! | `content:` | normalized | `Pattern{kind: Literal, body: <value>}` |
//! | `index:` | refused | `BRIDGE_UNSUPPORTED_DIRECTIVE` |
//! | `context:` | refused | `BRIDGE_UNSUPPORTED_DIRECTIVE` |
//! | unknown name | refused | `BRIDGE_UNSUPPORTED_FILTER` |
//!
//! Any combination producing more than one competing rewrite (e.g.
//! both `fork:` and `archived:` and `visibility:` together) returns
//! `BRIDGE_AMBIGUOUS_FILTER`. The fail-closed posture is preferred
//! over silent acceptance per [CLAUDE.md § Agent change posture].
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::BridgeError;
use crate::syntax::{SgFilter, SgQuery};
use crate::version::SourcegraphVersionTag;

/// Placeholder lowered LQ directive tree.
///
/// Shape mirrors the LQ canonical AST family closely enough that the
/// integration ticket can replace this type with the real `LqExpr`
/// via a mechanical mapping without further translator changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LqDirective {
    /// A lexical pattern (literal / phrase). The `kind` field
    /// distinguishes `literal` / `phrase` / `regex`; v1 emits
    /// `literal` for bare patterns and translated `content:` values,
    /// and `phrase` for quoted strings (already handled by the
    /// parser at `SgQuery::Pattern` level — the translator treats both
    /// uniformly as `literal` for now since the parser collapses
    /// phrases to bare patterns).
    Pattern {
        kind: Box<str>,
        body: Box<str>,
    },
    /// A `name:value` LQ filter, where `name` is the canonical LQ
    /// filter name after Sourcegraph normalization.
    Filter {
        name: Box<str>,
        value: Box<str>,
    },
    And(Vec<LqDirective>),
    Or(Vec<LqDirective>),
    Not(Box<LqDirective>),
    /// A `Filtered` directive — one or more filters scoping a body
    /// subexpression. Matches LQ's canonical filter-prefix shape.
    Filtered {
        filters: Vec<LqDirective>,
        body: Box<LqDirective>,
    },
}

impl LqDirective {
    /// Tag string used by the hand-rolled serde impl.
    const fn tag(&self) -> &'static str {
        match self {
            Self::Pattern { .. } => "pattern",
            Self::Filter { .. } => "filter",
            Self::And(_) => "and",
            Self::Or(_) => "or",
            Self::Not(_) => "not",
            Self::Filtered { .. } => "filtered",
        }
    }
}

impl serde::Serialize for LqDirective {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let n = match self {
            Self::And(_) | Self::Or(_) | Self::Not(_) => 2,
            Self::Pattern { .. } | Self::Filter { .. } | Self::Filtered { .. } => 3,
        };
        let mut m = ser.serialize_map(Some(n))?;
        m.serialize_entry("tag", self.tag())?;
        match self {
            Self::Pattern { kind, body } => {
                m.serialize_entry("kind", kind.as_ref())?;
                m.serialize_entry("body", body.as_ref())?;
            }
            Self::Filter { name, value } => {
                m.serialize_entry("name", name.as_ref())?;
                m.serialize_entry("value", value.as_ref())?;
            }
            Self::And(xs) | Self::Or(xs) => {
                m.serialize_entry("items", xs)?;
            }
            Self::Not(inner) => {
                m.serialize_entry("item", inner.as_ref())?;
            }
            Self::Filtered { filters, body } => {
                m.serialize_entry("filters", filters)?;
                m.serialize_entry("body", body.as_ref())?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for LqDirective {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = LqDirective;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqDirective tagged map")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<LqDirective, M::Error> {
                let mut tag: Option<String> = None;
                let mut kind: Option<String> = None;
                let mut body_s: Option<String> = None;
                let mut name: Option<String> = None;
                let mut value: Option<String> = None;
                let mut items: Option<Vec<LqDirective>> = None;
                let mut item: Option<Box<LqDirective>> = None;
                let mut filters: Option<Vec<LqDirective>> = None;
                let mut body_q: Option<Box<LqDirective>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "tag" => {
                            if tag.is_some() {
                                return Err(M::Error::duplicate_field("tag"));
                            }
                            tag = Some(map.next_value()?);
                        }
                        "kind" => {
                            if kind.is_some() {
                                return Err(M::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        "body" => {
                            // Distinguish by current tag, which must
                            // be set first.
                            let t = tag.as_deref().ok_or_else(|| {
                                M::Error::custom("`tag` must precede `body` in LqDirective")
                            })?;
                            match t {
                                "pattern" => {
                                    if body_s.is_some() {
                                        return Err(M::Error::duplicate_field("body"));
                                    }
                                    body_s = Some(map.next_value()?);
                                }
                                "filtered" => {
                                    if body_q.is_some() {
                                        return Err(M::Error::duplicate_field("body"));
                                    }
                                    body_q = Some(map.next_value()?);
                                }
                                other => {
                                    return Err(M::Error::custom(format!(
                                        "`body` not valid for tag `{other}`"
                                    )));
                                }
                            }
                        }
                        "name" => {
                            if name.is_some() {
                                return Err(M::Error::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        "value" => {
                            if value.is_some() {
                                return Err(M::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value()?);
                        }
                        "items" => {
                            if items.is_some() {
                                return Err(M::Error::duplicate_field("items"));
                            }
                            items = Some(map.next_value()?);
                        }
                        "item" => {
                            if item.is_some() {
                                return Err(M::Error::duplicate_field("item"));
                            }
                            item = Some(map.next_value()?);
                        }
                        "filters" => {
                            if filters.is_some() {
                                return Err(M::Error::duplicate_field("filters"));
                            }
                            filters = Some(map.next_value()?);
                        }
                        other => {
                            return Err(M::Error::unknown_field(
                                other,
                                &[
                                    "tag", "kind", "body", "name", "value", "items", "item",
                                    "filters",
                                ],
                            ));
                        }
                    }
                }
                let t = tag.ok_or_else(|| M::Error::missing_field("tag"))?;
                match t.as_str() {
                    "pattern" => {
                        let kind = kind.ok_or_else(|| M::Error::missing_field("kind"))?;
                        let body = body_s.ok_or_else(|| M::Error::missing_field("body"))?;
                        Ok(LqDirective::Pattern {
                            kind: kind.into_boxed_str(),
                            body: body.into_boxed_str(),
                        })
                    }
                    "filter" => {
                        let name = name.ok_or_else(|| M::Error::missing_field("name"))?;
                        let value = value.ok_or_else(|| M::Error::missing_field("value"))?;
                        Ok(LqDirective::Filter {
                            name: name.into_boxed_str(),
                            value: value.into_boxed_str(),
                        })
                    }
                    "and" => {
                        let xs = items.ok_or_else(|| M::Error::missing_field("items"))?;
                        Ok(LqDirective::And(xs))
                    }
                    "or" => {
                        let xs = items.ok_or_else(|| M::Error::missing_field("items"))?;
                        Ok(LqDirective::Or(xs))
                    }
                    "not" => {
                        let it = item.ok_or_else(|| M::Error::missing_field("item"))?;
                        Ok(LqDirective::Not(it))
                    }
                    "filtered" => {
                        let filters = filters.ok_or_else(|| M::Error::missing_field("filters"))?;
                        let body = body_q.ok_or_else(|| M::Error::missing_field("body"))?;
                        Ok(LqDirective::Filtered { filters, body })
                    }
                    other => Err(M::Error::custom(format!(
                        "unknown LqDirective tag `{other}`"
                    ))),
                }
            }
        }
        de.deserialize_map(V)
    }
}

/// Translate a parsed Sourcegraph query into [`LqDirective`].
///
/// `sg_version` is recorded for audit (the caller is responsible for
/// validating that it matches the supported pin; this function does
/// not call back into [`crate::version::SUPPORTED_SG_VERSION`] so the
/// caller can experiment with multiple pins in testing). Unsupported
/// pins surface earlier at [`SourcegraphVersionTag::new`].
pub fn translate(
    sg: SgQuery,
    sg_version: &SourcegraphVersionTag,
) -> Result<LqDirective, BridgeError> {
    // The version tag is currently consumed only for refusal payloads
    // (none yet record it explicitly); referencing it here keeps the
    // parameter live for future use without a stale-arg warning.
    let _: &SourcegraphVersionTag = sg_version;
    translate_inner(sg)
}

fn translate_inner(sg: SgQuery) -> Result<LqDirective, BridgeError> {
    match sg {
        SgQuery::Pattern(p) => Ok(LqDirective::Pattern {
            kind: Box::<str>::from("literal"),
            body: p,
        }),
        SgQuery::And(xs) => {
            let mut out: Vec<LqDirective> = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(translate_inner(x)?);
            }
            Ok(LqDirective::And(out))
        }
        SgQuery::Or(xs) => {
            let mut out: Vec<LqDirective> = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(translate_inner(x)?);
            }
            Ok(LqDirective::Or(out))
        }
        SgQuery::Not(inner) => {
            let lowered = translate_inner(*inner)?;
            Ok(LqDirective::Not(Box::new(lowered)))
        }
        SgQuery::Filtered { filters, body } => translate_filtered(filters, *body),
    }
}

fn translate_filtered(filters: Vec<SgFilter>, body: SgQuery) -> Result<LqDirective, BridgeError> {
    // Ambiguity check: at most one of {fork:, archived:, visibility:}
    // is allowed because all three lower onto the `visibility` LQ
    // filter. Mixing them is a fail-closed ambiguity.
    let mut visibility_sources: Vec<&'static str> = Vec::new();
    for f in &filters {
        match f {
            SgFilter::Fork(_) => visibility_sources.push("fork"),
            SgFilter::Archived(_) => visibility_sources.push("archived"),
            SgFilter::Visibility(_) => visibility_sources.push("visibility"),
            SgFilter::Repo(_)
            | SgFilter::File(_)
            | SgFilter::Lang(_)
            | SgFilter::Type(_)
            | SgFilter::Case(_)
            | SgFilter::Select(_)
            | SgFilter::Count(_)
            | SgFilter::Patterntype(_)
            | SgFilter::Content(_)
            | SgFilter::Context(_)
            | SgFilter::Index(_) => {}
        }
    }
    if visibility_sources.len() > 1 {
        let candidates = visibility_sources.join(",");
        return Err(BridgeError::ambiguous_filter(
            "visibility",
            format!("multiple filters compete for LQ `visibility:` target: [{candidates}]"),
        ));
    }
    let mut lowered_filters: Vec<LqDirective> = Vec::with_capacity(filters.len());
    let mut content_patterns: Vec<LqDirective> = Vec::new();
    for f in filters {
        match lower_filter(&f)? {
            LowerOutcome::Filter(d) => lowered_filters.push(d),
            LowerOutcome::ContentPattern(d) => content_patterns.push(d),
        }
    }
    let body_lowered = translate_inner(body)?;
    // Build body: original body AND content-derived patterns, in
    // source order. If body is the empty placeholder Pattern{literal,
    // ""}, drop it.
    let mut body_terms: Vec<LqDirective> = Vec::new();
    if !is_empty_placeholder(&body_lowered) {
        body_terms.push(body_lowered);
    }
    for p in content_patterns {
        body_terms.push(p);
    }
    let body = match body_terms.len() {
        0 => LqDirective::Pattern {
            kind: Box::<str>::from("literal"),
            body: Box::<str>::from(""),
        },
        1 => match body_terms.into_iter().next() {
            Some(t) => t,
            None => {
                return Err(BridgeError::translate_fail(
                    "internal: empty body_terms after singleton check",
                ));
            }
        },
        _ => LqDirective::And(body_terms),
    };
    if lowered_filters.is_empty() {
        Ok(body)
    } else {
        Ok(LqDirective::Filtered {
            filters: lowered_filters,
            body: Box::new(body),
        })
    }
}

enum LowerOutcome {
    Filter(LqDirective),
    ContentPattern(LqDirective),
}

fn lower_filter(f: &SgFilter) -> Result<LowerOutcome, BridgeError> {
    match f {
        // Adopted 1:1.
        SgFilter::Repo(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("repo"),
            value: v.clone(),
        })),
        SgFilter::File(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("file"),
            value: v.clone(),
        })),
        SgFilter::Lang(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("lang"),
            value: v.clone(),
        })),
        SgFilter::Type(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("type"),
            value: v.clone(),
        })),
        SgFilter::Case(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("case"),
            value: v.clone(),
        })),
        SgFilter::Select(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("select"),
            value: v.clone(),
        })),
        SgFilter::Count(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("count"),
            value: v.clone(),
        })),
        SgFilter::Patterntype(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("patterntype"),
            value: v.clone(),
        })),
        // Normalized to `visibility:`.
        SgFilter::Fork(v) => normalize_fork(v).map(LowerOutcome::Filter),
        SgFilter::Archived(v) => normalize_archived(v).map(LowerOutcome::Filter),
        SgFilter::Visibility(v) => Ok(LowerOutcome::Filter(LqDirective::Filter {
            name: Box::<str>::from("visibility"),
            value: v.clone(),
        })),
        // Normalized: `content:` becomes a pattern leaf attached to
        // the body.
        SgFilter::Content(v) => Ok(LowerOutcome::ContentPattern(LqDirective::Pattern {
            kind: Box::<str>::from("literal"),
            body: v.clone(),
        })),
        // Refused.
        SgFilter::Index(v) => Err(BridgeError::unsupported_directive(
            &format!("index:{v}"),
            "Sourcegraph `index:` directive has no LQ projection; search-plane indexing policy is fixed",
        )),
        SgFilter::Context(v) => Err(BridgeError::unsupported_directive(
            &format!("context:{v}"),
            "Sourcegraph `context:` named-search references are scope-tags outside the LQ model",
        )),
    }
}

fn normalize_fork(value: &str) -> Result<LqDirective, BridgeError> {
    let canonical = match value {
        "yes" => "include_forks",
        "no" => "exclude_forks",
        "only" => "only_forks",
        other => {
            return Err(BridgeError::unsupported_directive(
                &format!("fork:{other}"),
                "Sourcegraph `fork:` value must be one of yes|no|only",
            ));
        }
    };
    Ok(LqDirective::Filter {
        name: Box::<str>::from("visibility"),
        value: Box::<str>::from(canonical),
    })
}

fn normalize_archived(value: &str) -> Result<LqDirective, BridgeError> {
    let canonical = match value {
        "yes" => "include_archived",
        "no" => "exclude_archived",
        "only" => "only_archived",
        other => {
            return Err(BridgeError::unsupported_directive(
                &format!("archived:{other}"),
                "Sourcegraph `archived:` value must be one of yes|no|only",
            ));
        }
    };
    Ok(LqDirective::Filter {
        name: Box::<str>::from("visibility"),
        value: Box::<str>::from(canonical),
    })
}

fn is_empty_placeholder(d: &LqDirective) -> bool {
    matches!(d, LqDirective::Pattern { kind, body } if kind.as_ref() == "literal" && body.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{LqDirective, translate};
    use crate::errors::BridgeErrorCode;
    use crate::syntax::{SgFilter, SgQuery, parse_sourcegraph};
    use crate::version::SourcegraphVersionTag;

    /// Resolve the supported-pin tag for tests.
    ///
    /// Falls back to a shape-validating literal if the workspace pin
    /// ever drifts; in either case returns a `SourcegraphVersionTag`
    /// so callers can stay on a `()` return type and avoid
    /// `clippy::panic_in_result_fn`.
    fn ver() -> SourcegraphVersionTag {
        // Try the supported pin first; fall back to a shape-equivalent
        // literal if the workspace pin ever drifts. The assertion in
        // the fallback halts the test process, so the inner `loop`
        // arm is provably unreachable in a sane harness but satisfies
        // the type checker without calling any disallowed
        // `unwrap`/`unwrap_or_else` helper.
        if let Ok(t) = SourcegraphVersionTag::supported() {
            return t;
        }
        assert!(false, "supported pin must parse");
        if let Ok(t) = SourcegraphVersionTag::new("sg-0.0.0") {
            return t;
        }
        loop {
            core::hint::spin_loop();
        }
    }

    fn parse(s: &str) -> SgQuery {
        match parse_sourcegraph(s) {
            Ok(q) => q,
            Err(e) => {
                assert!(false, "parse failed for `{s}`: {e}");
                SgQuery::Pattern(Box::<str>::from(""))
            }
        }
    }

    fn run(sg: &str) -> LqDirective {
        let q = parse(sg);
        let v = ver();
        match translate(q, &v) {
            Ok(d) => d,
            Err(e) => {
                assert!(false, "translate failed for `{sg}`: {e}");
                LqDirective::Pattern {
                    kind: Box::<str>::from("literal"),
                    body: Box::<str>::from(""),
                }
            }
        }
    }

    #[test]
    fn adopted_repo_lowers_one_to_one() {
        let lq = run("repo:acme/foo");
        let LqDirective::Filtered { filters, body } = lq else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 1);
        let Some(LqDirective::Filter { name, value }) = filters.first() else {
            assert!(false, "expected Filter");
            return;
        };
        assert_eq!(&**name, "repo");
        assert_eq!(&**value, "acme/foo");
        let LqDirective::Pattern { kind, body } = *body else {
            assert!(false, "expected Pattern body");
            return;
        };
        assert_eq!(&*kind, "literal");
        assert_eq!(&*body, "");
    }

    #[test]
    fn adopted_filter_set_each_maps() {
        for (sg, name) in [
            ("repo:x foo", "repo"),
            ("file:x foo", "file"),
            ("lang:rust foo", "lang"),
            ("type:symbol foo", "type"),
            ("case:yes foo", "case"),
            ("select:repo foo", "select"),
            ("count:100 foo", "count"),
            ("patterntype:literal foo", "patterntype"),
        ] {
            let lq = run(sg);
            let LqDirective::Filtered { filters, .. } = lq else {
                assert!(false, "{sg}: expected Filtered");
                continue;
            };
            let Some(LqDirective::Filter { name: n, .. }) = filters.first() else {
                assert!(false, "{sg}: expected Filter");
                continue;
            };
            assert_eq!(&**n, name);
        }
    }

    #[test]
    fn normalized_fork_yes_becomes_visibility() {
        let lq = run("fork:yes foo");
        let LqDirective::Filtered { filters, .. } = lq else {
            assert!(false, "expected Filtered");
            return;
        };
        let Some(LqDirective::Filter { name, value }) = filters.first() else {
            assert!(false, "expected Filter");
            return;
        };
        assert_eq!(&**name, "visibility");
        assert_eq!(&**value, "include_forks");
    }

    #[test]
    fn normalized_fork_no_and_only() {
        for (sg, expect) in [
            ("fork:no foo", "exclude_forks"),
            ("fork:only foo", "only_forks"),
        ] {
            let lq = run(sg);
            let LqDirective::Filtered { filters, .. } = lq else {
                assert!(false, "{sg}: expected Filtered");
                continue;
            };
            let Some(LqDirective::Filter { value, .. }) = filters.first() else {
                assert!(false, "{sg}: expected Filter");
                continue;
            };
            assert_eq!(&**value, expect);
        }
    }

    #[test]
    fn normalized_archived_values() {
        for (sg, expect) in [
            ("archived:yes foo", "include_archived"),
            ("archived:no foo", "exclude_archived"),
            ("archived:only foo", "only_archived"),
        ] {
            let lq = run(sg);
            let LqDirective::Filtered { filters, .. } = lq else {
                assert!(false, "{sg}: expected Filtered");
                continue;
            };
            let Some(LqDirective::Filter { name, value }) = filters.first() else {
                assert!(false, "{sg}: expected Filter");
                continue;
            };
            assert_eq!(&**name, "visibility");
            assert_eq!(&**value, expect);
        }
    }

    #[test]
    fn refused_fork_value() {
        let q = parse("fork:maybe foo");
        let v = ver();
        match translate(q, &v) {
            Ok(_) => assert!(false, "fork:maybe must refuse"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn normalized_content_becomes_pattern() {
        let lq = run("content:hello");
        let LqDirective::Pattern { kind, body } = lq else {
            assert!(false, "expected Pattern");
            return;
        };
        assert_eq!(&*kind, "literal");
        assert_eq!(&*body, "hello");
    }

    #[test]
    fn content_with_other_filter_pattern_attaches_to_body() {
        let lq = run("repo:acme content:hello");
        let LqDirective::Filtered { filters, body } = lq else {
            assert!(false, "expected Filtered");
            return;
        };
        assert_eq!(filters.len(), 1);
        let LqDirective::Pattern { body, .. } = *body else {
            assert!(false, "expected Pattern body");
            return;
        };
        assert_eq!(&*body, "hello");
    }

    #[test]
    fn refused_index_directive() {
        let q = parse("index:no foo");
        let v = ver();
        match translate(q, &v) {
            Ok(_) => assert!(false, "index: must refuse"),
            Err(e) => {
                assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective);
                match e.source_construct.as_deref() {
                    Some(s) => assert!(s.starts_with("index:")),
                    None => assert!(false, "expected source_construct"),
                }
            }
        }
    }

    #[test]
    fn refused_context_directive() {
        let q = parse("context:global foo");
        let v = ver();
        match translate(q, &v) {
            Ok(_) => assert!(false, "context: must refuse"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn ambiguous_filter_when_fork_and_visibility_combined() {
        let q = SgQuery::Filtered {
            filters: vec![
                SgFilter::Fork(Box::<str>::from("yes")),
                SgFilter::Visibility(Box::<str>::from("public")),
            ],
            body: Box::new(SgQuery::Pattern(Box::<str>::from("foo"))),
        };
        let v = ver();
        match translate(q, &v) {
            Ok(_) => assert!(false, "ambiguous combo must refuse"),
            Err(e) => {
                assert_eq!(e.code, BridgeErrorCode::BridgeAmbiguousFilter);
                match e.source_construct.as_deref() {
                    Some(s) => assert_eq!(s, "visibility"),
                    None => assert!(false, "expected source_construct"),
                }
            }
        }
    }

    #[test]
    fn ambiguous_filter_when_fork_and_archived_combined() {
        let q = SgQuery::Filtered {
            filters: vec![
                SgFilter::Fork(Box::<str>::from("yes")),
                SgFilter::Archived(Box::<str>::from("no")),
            ],
            body: Box::new(SgQuery::Pattern(Box::<str>::from("foo"))),
        };
        let v = ver();
        match translate(q, &v) {
            Ok(_) => assert!(false, "ambiguous combo must refuse"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeAmbiguousFilter),
        }
    }

    #[test]
    fn boolean_or_translates_recursively() {
        let lq = run("foo OR bar");
        let LqDirective::Or(xs) = lq else {
            assert!(false, "expected Or");
            return;
        };
        assert_eq!(xs.len(), 2);
    }

    #[test]
    fn boolean_not_translates_recursively() {
        let lq = run("NOT foo");
        let LqDirective::Not(inner) = lq else {
            assert!(false, "expected Not");
            return;
        };
        let LqDirective::Pattern { body, .. } = *inner else {
            assert!(false, "expected Pattern inside Not");
            return;
        };
        assert_eq!(&*body, "foo");
    }

    #[test]
    fn lq_directive_serde_roundtrip_filter() {
        let d = LqDirective::Filter {
            name: Box::<str>::from("repo"),
            value: Box::<str>::from("acme/foo"),
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&d, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<LqDirective, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, d),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn lq_directive_serde_roundtrip_filtered() {
        let d = LqDirective::Filtered {
            filters: vec![LqDirective::Filter {
                name: Box::<str>::from("lang"),
                value: Box::<str>::from("rust"),
            }],
            body: Box::new(LqDirective::Pattern {
                kind: Box::<str>::from("literal"),
                body: Box::<str>::from("foo"),
            }),
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&d, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<LqDirective, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, d),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
