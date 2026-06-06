//! Sourcegraph → LQ lowering.
//!
//! [`translate_query`] is the one-way translator surface from
//! [BRIDGE-01](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/tickets/BRIDGE-01.md).
//! It lowers an already-parsed [`SgQuery`] directly into the canonical
//! `LqQuery` wire shape. There is no bridge-local placeholder AST and no
//! second compiler stage in `search-plane`; Sourcegraph syntax joins the same
//! canonical query family that native text syntax uses.
//!
//! ## Decision table (from
//! [BRIDGE-01 § 5 step 5 / § 6.1](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/tickets/BRIDGE-01.md))
//!
//! | Sourcegraph filter | Bucket | LQ lowering |
//! |---|---|---|
//! | `repo:` / `file:` / `path:` / `lang:` / `rev:` / `author:` / `committer:` / `message:` / `case:` / `select:` / `count:` / `type:` / `patterntype:` | adopted | 1:1 `LqFilter` |
//! | `dirty:` / `fork:` / `archived:` / `visibility:` / `context:` | adopted | active LQ filter surface |
//! | `content:` | normalized | `Pattern{kind: Literal, body: <value>}` |
//! | `index:` / `boost:` | adopted | canonical `LqOptions` carrier |
//! | `timeout:` | adopted | `LqOptions.timeout_ms` |
//! | `file:contains(...)` / `file:has.content(...)` | adopted | active LQ predicate leaf or executable pattern lowering downstream |
//! | remaining `repo:` / `file:` predicates | adopted | active LQ predicate leaf lowering downstream |
//! | unknown name | refused | `BRIDGE_UNSUPPORTED_FILTER` |
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use crate::errors::{BridgeError, BridgeErrorCode};
use crate::syntax::{SgFilter, SgQuery};
use crate::version::SourcegraphVersionTag;
use quanta_index_contract::{
    LQ_VERSION_TAG, LqCase, LqCountBound, LqDirective, LqExpr, LqFileScope, LqFilter, LqLeaf,
    LqOptions, LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqSpan, LqType, LqVisibility,
    LqYesNoOnly,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BridgeMetadata {
    filters: Vec<LqFilter>,
    directives: Vec<LqDirective>,
    pattern_type: Option<LqPatternType>,
    case: Option<LqCase>,
    count: Option<LqCountBound>,
    timeout_ms: Option<u64>,
    index_mode: Option<LqYesNoOnly>,
    boost_millis: Option<u32>,
}

impl BridgeMetadata {
    fn is_empty(&self) -> bool {
        self.filters.is_empty()
            && self.directives.is_empty()
            && self.pattern_type.is_none()
            && self.case.is_none()
            && self.count.is_none()
            && self.timeout_ms.is_none()
            && self.index_mode.is_none()
            && self.boost_millis.is_none()
    }

    fn merge_from(&mut self, other: Self) {
        self.filters.extend(other.filters);
        self.directives.extend(other.directives);
        if let Some(pattern_type) = other.pattern_type {
            self.pattern_type = Some(pattern_type);
        }
        if let Some(case) = other.case {
            self.case = Some(case);
        }
        if let Some(count) = other.count {
            self.count = Some(count);
        }
        if let Some(timeout_ms) = other.timeout_ms {
            self.timeout_ms = Some(timeout_ms);
        }
        if let Some(index_mode) = other.index_mode {
            self.index_mode = Some(index_mode);
        }
        if let Some(boost_millis) = other.boost_millis {
            self.boost_millis = Some(boost_millis);
        }
    }

    fn apply_to_query(self, query: &mut LqQuery) {
        query.filters.extend(self.filters);
        query.directives.extend(self.directives);
        if let Some(pattern_type) = self.pattern_type {
            query.options.pattern_type = pattern_type;
        }
        if let Some(case) = self.case {
            query.options.case = Some(case);
        }
        if let Some(count) = self.count {
            query.options.count = Some(count);
        }
        if let Some(timeout_ms) = self.timeout_ms {
            query.options.timeout_ms = Some(timeout_ms);
        }
        if let Some(index_mode) = self.index_mode {
            query.options.index_mode = Some(index_mode);
        }
        if let Some(boost_millis) = self.boost_millis {
            query.options.boost_millis = Some(boost_millis);
        }
    }
}

fn new_bridge_query(source_len: u32) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Empty,
        filters: Vec::new(),
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::eof(source_len),
    }
}

/// Translate a parsed Sourcegraph query directly into canonical `LqQuery`.
pub fn translate_query(
    sg: SgQuery,
    sg_version: &SourcegraphVersionTag,
    source_len: usize,
) -> Result<LqQuery, BridgeError> {
    let source_len = u32::try_from(source_len).map_err(|err| {
        BridgeError::translate_fail(format!("bridge: query length does not fit u32: {err}"))
    })?;
    let _: &SourcegraphVersionTag = sg_version;
    let mut query = new_bridge_query(source_len);
    let mut metadata = BridgeMetadata::default();
    query.expr = lower_sourcegraph_expr(sg, &mut metadata)?;
    metadata.apply_to_query(&mut query);
    Ok(query)
}

fn lower_sourcegraph_expr(
    sg: SgQuery,
    metadata: &mut BridgeMetadata,
) -> Result<LqExpr, BridgeError> {
    match sg {
        SgQuery::Pattern { kind, body } => Ok(lower_sourcegraph_pattern(kind, body.as_ref())),
        SgQuery::Predicate {
            scope,
            name,
            args_raw,
        } => lower_bridge_predicate(&format!("{scope}.{name}"), args_raw.as_ref()),
        SgQuery::And(xs) => {
            let mut out: Vec<LqExpr> = Vec::with_capacity(xs.len());
            for x in xs {
                let lowered = lower_sourcegraph_expr(x, metadata)?;
                if !matches!(lowered, LqExpr::Empty) {
                    out.push(lowered);
                }
            }
            Ok(collapse_exprs(out, true))
        }
        SgQuery::Or(xs) => lower_sourcegraph_or(xs, metadata),
        SgQuery::Not(inner) => lower_sourcegraph_not(*inner),
        SgQuery::Filtered { filters, body } => lower_sourcegraph_filtered(filters, *body, metadata),
    }
}

fn lower_sourcegraph_or(
    xs: Vec<SgQuery>,
    metadata: &mut BridgeMetadata,
) -> Result<LqExpr, BridgeError> {
    let mut out: Vec<LqExpr> = Vec::with_capacity(xs.len());
    let mut first_branch_metadata: Option<BridgeMetadata> = None;
    let mut identical_branch_metadata = true;
    let mut any_scoped_metadata = false;

    for x in xs {
        let mut branch_metadata = BridgeMetadata::default();
        let lowered = lower_sourcegraph_expr(x, &mut branch_metadata)?;
        if matches!(lowered, LqExpr::Empty) {
            return Err(BridgeError::translate_fail(
                "bridge: OR branch lowered to filters-only query, which the active LQ contract cannot represent",
            ));
        }
        any_scoped_metadata |= !branch_metadata.is_empty();
        if let Some(expected) = first_branch_metadata.as_ref() {
            if expected != &branch_metadata {
                identical_branch_metadata = false;
            }
        } else {
            first_branch_metadata = Some(branch_metadata.clone());
        }
        out.push(lowered);
    }

    if !identical_branch_metadata && any_scoped_metadata {
        return Err(BridgeError::translate_fail(
            "bridge: scoped filters under OR/NOT are not representable on the active LQ wire",
        ));
    }
    if identical_branch_metadata && let Some(branch_metadata) = first_branch_metadata {
        metadata.merge_from(branch_metadata);
    }
    Ok(collapse_exprs(out, false))
}

fn lower_sourcegraph_not(inner: SgQuery) -> Result<LqExpr, BridgeError> {
    let mut branch_metadata = BridgeMetadata::default();
    let lowered = lower_sourcegraph_expr(inner, &mut branch_metadata)?;
    if !branch_metadata.is_empty() {
        return Err(BridgeError::translate_fail(
            "bridge: scoped filters under OR/NOT are not representable on the active LQ wire",
        ));
    }
    if matches!(lowered, LqExpr::Empty) {
        return Err(BridgeError::translate_fail(
            "bridge: NOT over filters-only subtree cannot be lowered into the active LQ contract",
        ));
    }
    Ok(LqExpr::Not(Box::new(lowered)))
}

fn lower_sourcegraph_filtered(
    filters: Vec<SgFilter>,
    body: SgQuery,
    metadata: &mut BridgeMetadata,
) -> Result<LqExpr, BridgeError> {
    let mut content_patterns: Vec<LqExpr> = Vec::new();
    for f in filters {
        match lower_filter(&f, metadata)? {
            LowerOutcome::ContentPattern(expr) => content_patterns.push(expr),
            LowerOutcome::Drop => {}
        }
    }

    let body_lowered = lower_sourcegraph_expr(body, metadata)?;
    let mut body_terms: Vec<LqExpr> = Vec::new();
    if !matches!(body_lowered, LqExpr::Empty) {
        body_terms.push(body_lowered);
    }
    body_terms.extend(content_patterns);
    Ok(collapse_exprs(body_terms, true))
}

enum LowerOutcome {
    ContentPattern(LqExpr),
    Drop,
}

fn lower_filter(f: &SgFilter, metadata: &mut BridgeMetadata) -> Result<LowerOutcome, BridgeError> {
    match f {
        // Adopted 1:1.
        SgFilter::Repo(v) => apply_bridge_filter("repo", v, metadata),
        SgFilter::File(v) => apply_bridge_filter("file", v, metadata),
        SgFilter::Path(v) => apply_bridge_filter("path", v, metadata),
        SgFilter::Lang(v) => apply_bridge_filter("lang", v, metadata),
        SgFilter::Rev(v) => apply_bridge_filter("rev", v, metadata),
        SgFilter::Author(v) => apply_bridge_filter("author", v, metadata),
        SgFilter::Committer(v) => apply_bridge_filter("committer", v, metadata),
        SgFilter::Message(v) => apply_bridge_filter("message", v, metadata),
        SgFilter::Type(v) => apply_bridge_filter("type", v, metadata),
        SgFilter::Case(v) => apply_bridge_filter("case", v, metadata),
        SgFilter::Select(v) => apply_bridge_filter("select", v, metadata),
        SgFilter::Count(v) => apply_bridge_filter("count", v, metadata),
        SgFilter::Patterntype(v) => apply_bridge_filter("patterntype", v, metadata),
        SgFilter::Dirty(v) => {
            let value = validate_yes_no_only("dirty", v)?;
            apply_bridge_filter("dirty", value.as_ref(), metadata)
        }
        SgFilter::Changed(v) => apply_bridge_filter("changed", v, metadata),
        SgFilter::Stale(v) => apply_bridge_filter("stale", v, metadata),
        SgFilter::Snapshot(v) => apply_bridge_filter("snapshot", v, metadata),
        SgFilter::MetaOwner(v) => apply_bridge_filter("meta.owner", v, metadata),
        SgFilter::MetaService(v) => apply_bridge_filter("meta.service", v, metadata),
        SgFilter::MetaLayer(v) => apply_bridge_filter("meta.layer", v, metadata),
        SgFilter::MetaSurface(v) => apply_bridge_filter("meta.surface", v, metadata),
        SgFilter::Affected(v) => apply_bridge_filter("affected", v, metadata),
        SgFilter::InvalidatedBy(v) => apply_bridge_filter("invalidated_by", v, metadata),
        SgFilter::Fork(v) => {
            let value = validate_yes_no_only("fork", v)?;
            apply_bridge_filter("fork", value.as_ref(), metadata)
        }
        SgFilter::Archived(v) => {
            let value = validate_yes_no_only("archived", v)?;
            apply_bridge_filter("archived", value.as_ref(), metadata)
        }
        SgFilter::Visibility(v) => {
            let value = validate_visibility(v)?;
            apply_bridge_filter("visibility", value.as_ref(), metadata)
        }
        // Normalized: `content:` becomes a pattern leaf attached to
        // the body.
        SgFilter::Content(v) => Ok(LowerOutcome::ContentPattern(LqExpr::Leaf(LqLeaf::Keyword(
            v.to_string(),
        )))),
        SgFilter::Index(v) => apply_bridge_filter("index", v, metadata),
        SgFilter::Boost(v) => apply_bridge_filter("boost", v, metadata),
        SgFilter::Context(v) => apply_bridge_filter("context", v, metadata),
        SgFilter::Timeout(v) => apply_bridge_filter("timeout", v, metadata),
        SgFilter::Before(v) => apply_bridge_filter("before", v, metadata),
        SgFilter::After(v) => apply_bridge_filter("after", v, metadata),
        SgFilter::Since(v) => apply_bridge_filter("since", v, metadata),
        SgFilter::Until(v) => apply_bridge_filter("until", v, metadata),
        SgFilter::DiffAdded(v) => apply_bridge_filter("diff.added", v, metadata),
        SgFilter::DiffRemoved(v) => apply_bridge_filter("diff.removed", v, metadata),
        SgFilter::DiffTouched(v) => apply_bridge_filter("diff.touched", v, metadata),
    }
}

fn validate_yes_no_only(name: &str, value: &str) -> Result<Box<str>, BridgeError> {
    match value {
        "yes" | "no" | "only" => Ok(Box::<str>::from(value)),
        other => Err(BridgeError::unsupported_directive(
            &format!("{name}:{other}"),
            format!("Sourcegraph `{name}:` value must be one of yes|no|only"),
        )),
    }
}

fn validate_visibility(value: &str) -> Result<Box<str>, BridgeError> {
    match value {
        "public" | "private" | "any" | "include_forks" | "exclude_forks" | "only_forks"
        | "include_archived" | "exclude_archived" | "only_archived" => Ok(Box::<str>::from(value)),
        other => Err(BridgeError::unsupported_directive(
            &format!("visibility:{other}"),
            "Sourcegraph `visibility:` value must be one of public|private|any|include_forks|exclude_forks|only_forks|include_archived|exclude_archived|only_archived",
        )),
    }
}

fn collapse_exprs(items: Vec<LqExpr>, all: bool) -> LqExpr {
    match items.len() {
        0 => LqExpr::Empty,
        1 => items.into_iter().next().map_or(LqExpr::Empty, |item| item),
        _ if all => LqExpr::All(items),
        _ => LqExpr::Any(items),
    }
}

fn lower_sourcegraph_pattern(kind: crate::syntax::SgPatternKind, body: &str) -> LqExpr {
    if matches!(kind, crate::syntax::SgPatternKind::Literal) && body.is_empty() {
        return LqExpr::Empty;
    }
    let leaf = match kind {
        crate::syntax::SgPatternKind::Literal => LqLeaf::Keyword(body.to_string()),
        crate::syntax::SgPatternKind::Phrase => LqLeaf::Phrase(body.to_string()),
        crate::syntax::SgPatternKind::Regex => LqLeaf::Regex(body.to_string()),
    };
    LqExpr::Leaf(leaf)
}

enum BridgePredicateArg {
    Phrase(String),
    RawString(String),
    Bare(String),
}

fn lower_bridge_predicate(name: &str, args_raw: &str) -> Result<LqExpr, BridgeError> {
    let args = parse_bridge_predicate_args(args_raw)?;
    if let Some(executable) = lower_executable_bridge_predicate(name, &args) {
        return Ok(executable);
    }
    if let Some(aliased) = lower_repo_has_path_alias(name, &args) {
        return aliased;
    }
    Ok(LqExpr::Leaf(LqLeaf::Predicate {
        name: name.to_string(),
        args: args.into_iter().map(to_lq_predicate_arg).collect(),
    }))
}

fn lower_executable_bridge_predicate(name: &str, args: &[BridgePredicateArg]) -> Option<LqExpr> {
    match name {
        "file.contains" | "file.has.content" | "file.contains.content" if args.len() == 1 => {
            args.first().and_then(lower_file_content_predicate_arg)
        }
        _ => None,
    }
}

/// `repo:has.path(<pattern>)` is SG-only sugar for `repo:has.file(path:<pattern>)`.
///
/// The lexical executor already owns the repo-file gate semantics. Rewriting
/// here keeps `repo.has.file` as the single canonical repo-path predicate and
/// avoids introducing a second engine-side matcher family.
fn lower_repo_has_path_alias(
    name: &str,
    args: &[BridgePredicateArg],
) -> Option<Result<LqExpr, BridgeError>> {
    if name != "repo.has.path" {
        return None;
    }
    let pattern = match args {
        [
            BridgePredicateArg::Bare(value)
            | BridgePredicateArg::RawString(value)
            | BridgePredicateArg::Phrase(value),
        ] => value.clone(),
        _ => {
            return Some(Err(BridgeError::translate_fail(
                "bridge: repo:has.path expects a single path-pattern argument".to_string(),
            )));
        }
    };
    Some(Ok(LqExpr::Leaf(LqLeaf::Predicate {
        name: "repo.has.file".to_string(),
        args: vec![LqPredicateArg::Filter {
            name: "path".to_string(),
            value: pattern,
        }],
    })))
}

fn lower_file_content_predicate_arg(arg: &BridgePredicateArg) -> Option<LqExpr> {
    match arg {
        BridgePredicateArg::Phrase(text) => Some(LqExpr::Leaf(LqLeaf::Phrase(text.clone()))),
        BridgePredicateArg::RawString(text) => Some(LqExpr::Leaf(LqLeaf::RawString(text.clone()))),
        BridgePredicateArg::Bare(text) => {
            if text.split_once(':').is_some() {
                return None;
            }
            if let Some(regex) = strip_regex_delimiters(text) {
                return Some(LqExpr::Leaf(LqLeaf::Regex(regex.to_string())));
            }
            Some(LqExpr::Leaf(LqLeaf::Keyword(text.clone())))
        }
    }
}

fn strip_regex_delimiters(text: &str) -> Option<&str> {
    text.strip_prefix('/')
        .and_then(|trimmed| trimmed.strip_suffix('/'))
}

fn to_lq_predicate_arg(arg: BridgePredicateArg) -> LqPredicateArg {
    match arg {
        BridgePredicateArg::Phrase(value) => LqPredicateArg::Phrase(value),
        BridgePredicateArg::RawString(value) => LqPredicateArg::RawString(value),
        BridgePredicateArg::Bare(value) => classify_bare_predicate_arg(value),
    }
}

fn classify_bare_predicate_arg(value: String) -> LqPredicateArg {
    if let Some((name, value)) = value.split_once(':') {
        return LqPredicateArg::Filter {
            name: name.to_string(),
            value: value.to_string(),
        };
    }
    if let Ok(number) = value.parse::<i64>() {
        return LqPredicateArg::Number(number);
    }
    LqPredicateArg::Keyword(value)
}

fn parse_bridge_predicate_args(raw: &str) -> Result<Vec<BridgePredicateArg>, BridgeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let bytes = raw.as_bytes();
    let mut pos: usize = 0;
    let mut out: Vec<BridgePredicateArg> = Vec::new();
    while pos < bytes.len() {
        while let Some(&b) = bytes.get(pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                pos = pos.saturating_add(1);
            } else {
                break;
            }
        }
        if pos >= bytes.len() {
            break;
        }

        let arg = match bytes.get(pos).copied() {
            Some(b'"') => {
                let (consumed, value) = read_quoted_bridge_arg(bytes, pos, b'"')?;
                pos = consumed;
                BridgePredicateArg::Phrase(value)
            }
            Some(b'\'') => {
                let (consumed, value) = read_quoted_bridge_arg(bytes, pos, b'\'')?;
                pos = consumed;
                BridgePredicateArg::RawString(value)
            }
            Some(_) => {
                let (consumed, value) = read_bare_bridge_arg(bytes, pos)?;
                pos = consumed;
                let value = value.trim();
                if value.is_empty() {
                    return Err(predicate_arg_error("bridge: empty predicate argument"));
                }
                BridgePredicateArg::Bare(value.to_string())
            }
            None => break,
        };
        out.push(arg);

        while let Some(&b) = bytes.get(pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                pos = pos.saturating_add(1);
            } else {
                break;
            }
        }
        if pos >= bytes.len() {
            break;
        }
        match bytes.get(pos).copied() {
            Some(b',') => pos = pos.saturating_add(1),
            Some(_) => {
                return Err(predicate_arg_error(
                    "bridge: unexpected character while parsing predicate arguments",
                ));
            }
            None => break,
        }
    }

    Ok(out)
}

fn read_quoted_bridge_arg(
    bytes: &[u8],
    start: usize,
    terminator: u8,
) -> Result<(usize, String), BridgeError> {
    let mut pos = start.saturating_add(1);
    let mut buf = String::new();
    while let Some(&b) = bytes.get(pos) {
        if b == terminator {
            return Ok((pos.saturating_add(1), buf));
        }
        if b == b'\\' && terminator == b'"' {
            let after = pos.saturating_add(1);
            let Some(&esc) = bytes.get(after) else {
                return Err(predicate_arg_error(
                    "bridge: trailing backslash in predicate phrase argument",
                ));
            };
            let mapped = match esc {
                b'\\' => '\\',
                b'"' => '"',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                _ => {
                    return Err(predicate_arg_error(
                        "bridge: unknown escape in predicate phrase argument",
                    ));
                }
            };
            buf.push(mapped);
            pos = after.saturating_add(1);
            continue;
        }
        let rest = bytes.get(pos..).unwrap_or(&[]);
        let ch = next_utf8_char(rest)?;
        buf.push(ch);
        pos = pos.saturating_add(ch.len_utf8());
    }
    Err(predicate_arg_error(
        "bridge: unterminated quoted predicate argument",
    ))
}

fn read_bare_bridge_arg(bytes: &[u8], start: usize) -> Result<(usize, String), BridgeError> {
    let mut pos = start;
    let mut buf = String::new();
    while let Some(&b) = bytes.get(pos) {
        if b == b',' {
            break;
        }
        let rest = bytes.get(pos..).unwrap_or(&[]);
        let ch = next_utf8_char(rest)?;
        buf.push(ch);
        pos = pos.saturating_add(ch.len_utf8());
    }
    Ok((pos, buf))
}

fn next_utf8_char(bytes: &[u8]) -> Result<char, BridgeError> {
    let text = core::str::from_utf8(bytes).map_err(|err| {
        BridgeError::translate_fail(format!(
            "bridge: invalid UTF-8 in predicate argument: {err}"
        ))
    })?;
    text.chars()
        .next()
        .ok_or_else(|| predicate_arg_error("bridge: invalid UTF-8 in predicate argument"))
}

fn predicate_arg_error(message: &str) -> BridgeError {
    BridgeError::translate_fail(message)
}

fn apply_bridge_filter(
    name: &str,
    value: &str,
    metadata: &mut BridgeMetadata,
) -> Result<LowerOutcome, BridgeError> {
    match name {
        "repo" => metadata.filters.push(LqFilter::Repo {
            pattern: value.to_string(),
            revs: Vec::new(),
        }),
        "file" => metadata.filters.push(LqFilter::File {
            pattern: value.to_string(),
            scope: LqFileScope::NameAndPath,
        }),
        "path" => metadata.filters.push(LqFilter::File {
            pattern: value.to_string(),
            scope: LqFileScope::PathOnly,
        }),
        "lang" => metadata.filters.push(LqFilter::Lang {
            id: value.to_string(),
        }),
        "rev" => metadata.filters.push(LqFilter::Rev {
            spec: value.to_string(),
        }),
        "author" => metadata.filters.push(LqFilter::Author {
            pattern: value.to_string(),
        }),
        "committer" => metadata.filters.push(LqFilter::Committer {
            pattern: value.to_string(),
        }),
        "message" => metadata.filters.push(LqFilter::Message {
            pattern: value.to_string(),
        }),
        "type" => {
            let kind = match value {
                "file" => LqType::File,
                "path" => LqType::Path,
                "symbol" => LqType::Symbol,
                "commit" => LqType::Commit,
                "diff" => LqType::Diff,
                "repo" => LqType::Repo,
                other => {
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("type:{other}").into_boxed_str()),
                        format!("bridge: unsupported Sourcegraph type filter `{other}`"),
                    ));
                }
            };
            metadata.filters.push(LqFilter::Type { kind });
        }
        "select" => {
            let dim = match value {
                "repo" => LqSelect::Repo,
                "file" => LqSelect::File,
                "file.owners" => LqSelect::FileOwners,
                "path" => LqSelect::Path,
                "symbol" => LqSelect::Symbol,
                "content" => LqSelect::Content,
                "content.match" => LqSelect::ContentMatch,
                other => {
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("select:{other}").into_boxed_str()),
                        format!("bridge: unsupported Sourcegraph select filter `{other}`"),
                    ));
                }
            };
            metadata.filters.push(LqFilter::Select { dim });
        }
        "dirty" => metadata.filters.push(LqFilter::Dirty {
            mode: lower_yes_no_only_filter("dirty", value)?,
        }),
        "changed" => metadata.filters.push(LqFilter::Changed {
            scope: value.to_string(),
        }),
        "stale" => metadata.filters.push(LqFilter::Stale {
            scope: value.to_string(),
        }),
        "snapshot" => metadata.filters.push(LqFilter::Snapshot {
            name: value.to_string(),
        }),
        "meta.owner" => metadata.filters.push(LqFilter::MetaOwner {
            id: value.to_string(),
        }),
        "meta.service" => metadata.filters.push(LqFilter::MetaService {
            id: value.to_string(),
        }),
        "meta.layer" => metadata.filters.push(LqFilter::MetaLayer {
            id: value.to_string(),
        }),
        "meta.surface" => metadata.filters.push(LqFilter::MetaSurface {
            id: value.to_string(),
        }),
        "affected" => metadata.filters.push(LqFilter::Affected {
            scope: value.to_string(),
        }),
        "invalidated_by" => metadata.filters.push(LqFilter::InvalidatedBy {
            source: value.to_string(),
        }),
        "case" => {
            metadata.case = Some(match value {
                "yes" => LqCase::Sensitive,
                "no" => LqCase::Insensitive,
                other => {
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("case:{other}").into_boxed_str()),
                        format!("bridge: unsupported Sourcegraph case filter `{other}`"),
                    ));
                }
            });
        }
        "count" => {
            metadata.count = Some(if value == "all" {
                LqCountBound::All
            } else {
                let parsed = value.parse::<u32>().map_err(|err| {
                    BridgeError::translate_fail(format!("bridge: invalid count `{value}`: {err}"))
                })?;
                LqCountBound::Bounded(parsed)
            });
        }
        "timeout" => {
            metadata.timeout_ms = Some(parse_timeout_ms(value)?);
        }
        "index" => {
            metadata.index_mode = Some(lower_yes_no_only_filter("index", value)?);
        }
        "boost" => {
            metadata.boost_millis = Some(parse_boost_millis(value)?);
        }
        "patterntype" => {
            metadata.pattern_type = Some(match value {
                "literal" => LqPatternType::Literal,
                "keyword" => LqPatternType::Keyword,
                "standard" => LqPatternType::Standard,
                "regexp" => LqPatternType::Regexp,
                "structural" => LqPatternType::Structural,
                other => {
                    return Err(BridgeError::new(
                        BridgeErrorCode::BridgeUnsupportedFilter,
                        Some(format!("patterntype:{other}").into_boxed_str()),
                        format!("bridge: unsupported patterntype `{other}`"),
                    ));
                }
            });
        }
        "fork" => metadata.filters.push(LqFilter::Fork {
            mode: lower_yes_no_only_filter("fork", value)?,
        }),
        "archived" => metadata.filters.push(LqFilter::Archived {
            mode: lower_yes_no_only_filter("archived", value)?,
        }),
        "visibility" => metadata.filters.push(lower_visibility_filter(value)?),
        "context" => metadata.filters.push(LqFilter::Context {
            name: value.to_string(),
        }),
        "before" => metadata.filters.push(LqFilter::Before {
            timeref: value.to_string(),
        }),
        "after" => metadata.filters.push(LqFilter::After {
            timeref: value.to_string(),
        }),
        "since" => metadata.filters.push(LqFilter::Since {
            timeref: value.to_string(),
        }),
        "until" => metadata.filters.push(LqFilter::Until {
            timeref: value.to_string(),
        }),
        "diff.added" => metadata.filters.push(LqFilter::DiffAdded {
            pattern: value.to_string(),
        }),
        "diff.removed" => metadata.filters.push(LqFilter::DiffRemoved {
            pattern: value.to_string(),
        }),
        "diff.touched" => metadata.filters.push(LqFilter::DiffTouched {
            pattern: value.to_string(),
        }),
        other => {
            return Err(BridgeError::unsupported_filter(
                other,
                format!("bridge: unsupported filter `{other}`"),
            ));
        }
    }
    Ok(LowerOutcome::Drop)
}

fn parse_timeout_ms(value: &str) -> Result<u64, BridgeError> {
    let (digits, unit) = split_timeout_value(value).ok_or_else(|| {
        BridgeError::translate_fail(format!(
            "bridge: invalid timeout `{value}`: expected <int><unit> with unit in {{ms,s,m,h}}"
        ))
    })?;
    let magnitude: u64 = digits.parse().map_err(|err| {
        BridgeError::translate_fail(format!("bridge: invalid timeout `{value}`: {err}"))
    })?;
    let multiplier: u64 = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => {
            return Err(BridgeError::translate_fail(format!(
                "bridge: invalid timeout `{value}`: unit must be one of {{ms,s,m,h}}"
            )));
        }
    };
    magnitude.checked_mul(multiplier).ok_or_else(|| {
        BridgeError::translate_fail(format!(
            "bridge: invalid timeout `{value}`: duration exceeds u64 milliseconds"
        ))
    })
}

fn parse_boost_millis(value: &str) -> Result<u32, BridgeError> {
    let parsed: f64 = value.parse().map_err(|err| {
        BridgeError::translate_fail(format!("bridge: invalid boost `{value}`: {err}"))
    })?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(BridgeError::translate_fail(format!(
            "bridge: invalid boost `{value}`: value must be a positive decimal"
        )));
    }
    let scaled = (parsed * 1000.0).round();
    if !scaled.is_finite() || scaled <= 0.0 || scaled > f64::from(u32::MAX) {
        return Err(BridgeError::translate_fail(format!(
            "bridge: invalid boost `{value}`: exceeds canonical precision/range"
        )));
    }
    Ok(scaled as u32)
}

fn split_timeout_value(value: &str) -> Option<(&str, &str)> {
    let digit_len = value.bytes().take_while(u8::is_ascii_digit).count();
    if digit_len == 0 || digit_len == value.len() {
        return None;
    }
    Some(value.split_at(digit_len))
}

fn lower_yes_no_only_filter(name: &str, value: &str) -> Result<LqYesNoOnly, BridgeError> {
    match value {
        "yes" => Ok(LqYesNoOnly::Yes),
        "no" => Ok(LqYesNoOnly::No),
        "only" => Ok(LqYesNoOnly::Only),
        other => Err(BridgeError::new(
            BridgeErrorCode::BridgeUnsupportedFilter,
            Some(format!("{name}:{other}").into_boxed_str()),
            format!("bridge: unsupported Sourcegraph {name} filter `{other}`"),
        )),
    }
}

fn lower_visibility_filter(value: &str) -> Result<LqFilter, BridgeError> {
    match value {
        "public" => Ok(LqFilter::Visibility {
            mode: LqVisibility::Public,
        }),
        "private" => Ok(LqFilter::Visibility {
            mode: LqVisibility::Private,
        }),
        "any" => Ok(LqFilter::Visibility {
            mode: LqVisibility::Any,
        }),
        "include_forks" => Ok(LqFilter::Fork {
            mode: LqYesNoOnly::Yes,
        }),
        "exclude_forks" => Ok(LqFilter::Fork {
            mode: LqYesNoOnly::No,
        }),
        "only_forks" => Ok(LqFilter::Fork {
            mode: LqYesNoOnly::Only,
        }),
        "include_archived" => Ok(LqFilter::Archived {
            mode: LqYesNoOnly::Yes,
        }),
        "exclude_archived" => Ok(LqFilter::Archived {
            mode: LqYesNoOnly::No,
        }),
        "only_archived" => Ok(LqFilter::Archived {
            mode: LqYesNoOnly::Only,
        }),
        other => Err(BridgeError::new(
            BridgeErrorCode::BridgeUnsupportedFilter,
            Some(format!("visibility:{other}").into_boxed_str()),
            format!("bridge: unsupported Sourcegraph visibility filter `{other}`"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::translate_query;
    use crate::errors::BridgeErrorCode;
    use crate::syntax::{SgPatternKind, SgQuery, parse_sourcegraph};
    use crate::version::SourcegraphVersionTag;
    use quanta_index_contract::{
        LQ_VERSION_TAG, LqCase, LqCountBound, LqExpr, LqFilter, LqLeaf, LqOptions, LqPatternType,
        LqPredicateArg, LqQuery, LqSpan, LqVisibility, LqYesNoOnly,
    };

    fn ver() -> SourcegraphVersionTag {
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
                SgQuery::Pattern {
                    kind: SgPatternKind::Literal,
                    body: Box::<str>::from(""),
                }
            }
        }
    }

    fn run(sg: &str) -> LqQuery {
        match translate_query(parse(sg), &ver(), sg.len()) {
            Ok(query) => query,
            Err(e) => {
                assert!(false, "translate failed for `{sg}`: {e}");
                LqQuery {
                    lq_version: LQ_VERSION_TAG,
                    expr: LqExpr::Empty,
                    filters: Vec::new(),
                    directives: Vec::new(),
                    options: LqOptions::defaults(),
                    source_span: LqSpan::eof(0),
                }
            }
        }
    }

    #[test]
    fn adopted_repo_only_becomes_filter_only_query() {
        let lq = run("repo:acme/foo");
        assert_eq!(lq.expr, LqExpr::Empty);
        assert_eq!(
            lq.filters,
            vec![LqFilter::Repo {
                pattern: "acme/foo".to_string(),
                revs: Vec::new(),
            }]
        );
    }

    #[test]
    fn case_count_timeout_and_patterntype_lower_into_canonical_options() {
        let lq = run("case:yes count:all timeout:5s patterntype:regexp foo");
        assert_eq!(lq.expr, LqExpr::Leaf(LqLeaf::Keyword("foo".to_string())));
        assert_eq!(lq.options.case, Some(LqCase::Sensitive));
        assert_eq!(lq.options.count, Some(LqCountBound::All));
        assert_eq!(lq.options.timeout_ms, Some(5_000));
        assert_eq!(lq.options.pattern_type, LqPatternType::Regexp);
    }

    #[test]
    fn fork_archived_visibility_and_context_preserve_canonical_filters() {
        let lq = run("fork:yes archived:no visibility:public context:global foo");
        assert_eq!(lq.expr, LqExpr::Leaf(LqLeaf::Keyword("foo".to_string())));
        assert_eq!(
            lq.filters,
            vec![
                LqFilter::Fork {
                    mode: LqYesNoOnly::Yes,
                },
                LqFilter::Archived {
                    mode: LqYesNoOnly::No,
                },
                LqFilter::Visibility {
                    mode: LqVisibility::Public,
                },
                LqFilter::Context {
                    name: "global".to_string(),
                },
            ]
        );
    }

    #[test]
    fn refused_fork_value() {
        match translate_query(parse("fork:maybe foo"), &ver(), "fork:maybe foo".len()) {
            Ok(_) => assert!(false, "fork:maybe must refuse"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn normalized_content_becomes_keyword_leaf() {
        let lq = run("content:hello");
        assert_eq!(lq.expr, LqExpr::Leaf(LqLeaf::Keyword("hello".to_string())));
    }

    #[test]
    fn content_with_body_collapses_into_and_expression() {
        let lq = run("repo:acme content:hello world");
        assert_eq!(
            lq.filters,
            vec![LqFilter::Repo {
                pattern: "acme".to_string(),
                revs: Vec::new(),
            }]
        );
        assert_eq!(
            lq.expr,
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("world".to_string())),
                LqExpr::Leaf(LqLeaf::Keyword("hello".to_string())),
            ])
        );
    }

    #[test]
    fn index_filter_lowers_into_canonical_option() {
        let lowered = run("index:no foo");
        assert_eq!(lowered.options.index_mode, Some(LqYesNoOnly::No));
        for (sg, expected) in [
            ("index:yes foo", LqYesNoOnly::Yes),
            ("index:only foo", LqYesNoOnly::Only),
        ] {
            let lowered = run(sg);
            assert_eq!(lowered.options.index_mode, Some(expected));
            assert_eq!(
                lowered.expr,
                LqExpr::Leaf(LqLeaf::Keyword("foo".to_string()))
            );
        }
    }

    #[test]
    fn boost_lowers_into_canonical_option() {
        let lowered = run("boost:5 foo");
        assert_eq!(lowered.options.boost_millis, Some(5000));
    }

    #[test]
    fn timeout_lowers_into_canonical_option() {
        let lowered = run("timeout:0ms /foo.*/");
        assert_eq!(lowered.options.timeout_ms, Some(0));
    }

    #[test]
    fn invalid_timeout_is_translate_fail() {
        match translate_query(parse("timeout:soon foo"), &ver(), "timeout:soon foo".len()) {
            Ok(_) => assert!(false, "timeout:soon foo must fail"),
            Err(e) => {
                assert_eq!(e.code, BridgeErrorCode::BridgeTranslateFail);
                assert!(e.detail.contains("invalid timeout"));
            }
        }
    }

    #[test]
    fn invalid_visibility_value_is_refused() {
        match translate_query(
            parse("visibility:team foo"),
            &ver(),
            "visibility:team foo".len(),
        ) {
            Ok(_) => assert!(false, "visibility:team must refuse"),
            Err(e) => assert_eq!(e.code, BridgeErrorCode::BridgeUnsupportedDirective),
        }
    }

    #[test]
    fn repo_predicate_lowers_to_predicate_leaf() {
        let lowered = run("repo:has.file(path:src/lib.rs)");
        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args: vec![LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/lib.rs".to_string(),
                }],
            })
        );
    }

    #[test]
    fn repo_contains_file_alias_preserves_predicate_leaf() {
        // Sourcegraph `repo:contains.file(...)` is preserved by the bridge as a
        // predicate leaf; the lexical executor canonicalizes the alias onto
        // `repo.has.file`. The bridge forwards the full matcher surface.
        let lowered = run("repo:contains.file(name:lib.rs)");
        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.contains.file".to_string(),
                args: vec![LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "lib.rs".to_string(),
                }],
            })
        );
    }

    #[test]
    fn repo_contains_path_alias_preserves_predicate_leaf() {
        let lowered = run("repo:contains.path(src/lib.rs)");
        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.contains.path".to_string(),
                args: vec![LqPredicateArg::Keyword("src/lib.rs".to_string())],
            })
        );
    }

    #[test]
    fn file_predicate_lowers_to_executable_phrase_leaf() {
        let lowered = run(r#"file:contains("TODO")"#);
        assert_eq!(
            lowered.expr,
            LqExpr::Leaf(LqLeaf::Phrase("TODO".to_string()))
        );
    }

    #[test]
    fn boolean_or_translates_recursively() {
        let lq = run("foo OR bar");
        assert_eq!(
            lq.expr,
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("foo".to_string())),
                LqExpr::Leaf(LqLeaf::Keyword("bar".to_string())),
            ])
        );
    }

    #[test]
    fn boolean_not_translates_recursively() {
        let lq = run("NOT foo");
        assert_eq!(
            lq.expr,
            LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("foo".to_string()))))
        );
    }

    #[test]
    fn phrase_and_regex_preserve_pattern_kind() {
        let phrase = run("\"hello world\"");
        assert_eq!(
            phrase.expr,
            LqExpr::Leaf(LqLeaf::Phrase("hello world".to_string()))
        );

        let regex = run("/h.llo/");
        assert_eq!(regex.expr, LqExpr::Leaf(LqLeaf::Regex("h.llo".to_string())));
    }

    #[test]
    fn identical_filtered_or_branches_are_hoisted() {
        let lowered = run("repo:acme/foo alpha OR repo:acme/foo beta");
        assert_eq!(
            lowered.filters,
            vec![LqFilter::Repo {
                pattern: "acme/foo".to_string(),
                revs: Vec::new(),
            }]
        );
        assert_eq!(
            lowered.expr,
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("alpha".to_string())),
                LqExpr::Leaf(LqLeaf::Keyword("beta".to_string())),
            ])
        );
    }

    #[test]
    fn differing_scoped_filters_under_or_fail_closed() {
        match translate_query(
            parse("repo:acme/foo alpha OR file:src beta"),
            &ver(),
            "repo:acme/foo alpha OR file:src beta".len(),
        ) {
            Ok(_) => assert!(false, "scoped filters under OR must refuse"),
            Err(e) => {
                assert_eq!(e.code, BridgeErrorCode::BridgeTranslateFail);
                assert!(e.detail.contains("scoped filters under OR/NOT"));
            }
        }
    }

    #[test]
    fn identical_standard_patterntype_branches_hoist_explicit_option() {
        let lowered = run("patterntype:standard alpha OR patterntype:standard beta");
        assert_eq!(lowered.options.pattern_type, LqPatternType::Standard);
        assert_eq!(
            lowered.expr,
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("alpha".to_string())),
                LqExpr::Leaf(LqLeaf::Keyword("beta".to_string())),
            ])
        );
    }
}
