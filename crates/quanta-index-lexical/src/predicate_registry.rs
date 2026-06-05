//! Predicate capability registry — the single source of truth for which
//! lexical predicate leaves are executable on the Tantivy adapter, what
//! argument shapes they admit, and which lowering target they resolve to.
//!
//! Before this module, predicate support was a set of hardcoded `match name`
//! arms duplicated across lexical lowering ([`crate`] `lib.rs`) and the
//! planner ([`crate::planner`]). Adding or widening a predicate meant editing
//! four string-match sites in lockstep — an Open/Closed violation that drifted
//! easily. This module lifts the name → capability mapping into one table so
//! callers dispatch on a typed [`PredicateKind`] instead of re-matching raw
//! names, and so the repo-file matcher argument contract is validated in one
//! place.
//!
//! Scope: this registry owns the **lexical content / repo-file** predicate
//! families (`file.contains`, `file.has.content`, `repo.has.file`). The
//! symbol-route predicate `symbol.has.name` is intentionally **not** here — it
//! lowers through the symbol planner, not the lexical content/repo seam, and
//! the planner handles it on a dedicated arm before consulting this registry.
//!
//! Adding a predicate is a registry-row change: append a [`PredicateSpec`] and,
//! only if it introduces a genuinely new lowering shape, one new
//! [`PredicateKind`] variant with its dispatch arm. Unsupported names continue
//! to typed-fail through [`unimplemented_predicate`] /
//! [`PREDICATE_UNIMPLEMENTED_CODE`].

use quanta_index_contract::{LqFileScope, LqPredicateArg};
use quanta_index_core::CoreError;

/// Stable typed-reject code for predicate shapes outside the executable
/// subset. Kept as the single owner of this wire code so diagnostics cannot
/// drift between lowering sites.
pub(crate) const PREDICATE_UNIMPLEMENTED_CODE: &str = "LEX_PREDICATE_UNIMPLEMENTED";

/// Follow-up owner referenced by predicate typed-reject diagnostics.
pub(crate) const PREDICATE_OWNER: &str = "LXE-03-predicate-extensions";
pub(crate) const PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED_CODE: &str =
    "LEX_PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED";

/// Lowering target classification for a registered predicate.
///
/// The variant selects *how* a predicate leaf is lowered, decoupling the
/// dispatch from the predicate name. Lowering sites match on this instead of
/// re-matching raw name strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PredicateKind {
    /// `file.contains` / `file.has.content` — exactly one content scalar
    /// argument, lowered to a lexical content leaf (keyword / phrase / regex /
    /// raw-substring). The per-argument lowering lives in `lib.rs`
    /// (`predicate_content_leaf`) because it depends on regex-delimiter
    /// stripping owned there. The admitted scalar contract is registry-owned
    /// via [`parse_content_predicate_constraint`], including numeric
    /// canonicalization and optional `file:` / `path:` / `lang:` scopes.
    ContentLeaf,
    /// `repo.has.file` — either a single textual scalar path shorthand or one
    /// or more `path:` / `name:` / `lang:` matcher filters, lowered to a
    /// repo-existence gate. Matcher parsing is registry-owned via
    /// [`parse_repo_file_matchers`].
    RepoFileGate,
    /// `repo.has.content` — exactly one content scalar (keyword / phrase /
    /// raw string / number), lowered to a repo-existence gate evaluated
    /// against indexed content. Unlike [`PredicateKind::ContentLeaf`] (which
    /// projects the matching *paths*) this gates the *repo* surface. Argument
    /// parsing is registry-owned via [`parse_content_scalar_arg`].
    RepoContentGate,
    /// `repo.has.commit.after` — exactly one timeref scalar, lowered to a
    /// repo-existence gate evaluated against source-repo keyed commit-recency
    /// authority materialized alongside the lexical generation.
    RepoCommitRecencyGate,
    /// `repo.has.meta` — exactly one `key:value` filter argument, lowered to a
    /// repo-existence gate evaluated against source-repo keyed repo-metadata
    /// authority materialized alongside the lexical generation. Argument parsing
    /// is registry-owned via [`parse_repo_meta_arg`].
    RepoMetaGate,
    /// `repo.has.topic` — exactly one textual topic scalar, lowered to a
    /// repo-existence gate evaluated against source-repo keyed repo-topic
    /// authority materialized alongside the lexical generation.
    RepoTopicGate,
    /// `file.has.owner` — zero args (`any owner`) or exactly one textual owner
    /// identity, lowered to a file-level candidate restriction evaluated
    /// against source-repo/path keyed ownership authority materialized
    /// alongside the lexical generation.
    FileOwnerGate,
    /// `file.has.contributor` — exactly one textual contributor identity,
    /// lowered to a file-level candidate restriction evaluated against
    /// source-repo/path keyed contributor authority materialized alongside the
    /// lexical generation.
    FileContributorGate,
}

/// One predicate capability row.
pub(crate) struct PredicateSpec {
    /// Canonical dot-joined predicate name (e.g. `repo.has.file`).
    pub name: &'static str,
    /// Lowering target for this predicate.
    pub kind: PredicateKind,
}

/// Native alias that rewrites onto one canonical executable predicate row.
pub(crate) struct PredicateAliasSpec {
    pub alias: &'static str,
    pub canonical: &'static str,
    pub rewrite: PredicateAliasRewrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PredicateAliasRewrite {
    IdentityArgs,
    RepoHasPathScalarToPathFilter,
}

/// The executable lexical predicate subset. This is the only place predicate
/// names are enumerated for the content/repo families.
pub(crate) const PREDICATE_REGISTRY: &[PredicateSpec] = &[
    PredicateSpec {
        name: "file.contains",
        kind: PredicateKind::ContentLeaf,
    },
    PredicateSpec {
        name: "file.has.content",
        kind: PredicateKind::ContentLeaf,
    },
    PredicateSpec {
        name: "repo.has.file",
        kind: PredicateKind::RepoFileGate,
    },
    PredicateSpec {
        name: "repo.has.content",
        kind: PredicateKind::RepoContentGate,
    },
    PredicateSpec {
        name: "repo.has.commit.after",
        kind: PredicateKind::RepoCommitRecencyGate,
    },
    PredicateSpec {
        name: "repo.has.meta",
        kind: PredicateKind::RepoMetaGate,
    },
    PredicateSpec {
        name: "repo.has.topic",
        kind: PredicateKind::RepoTopicGate,
    },
    PredicateSpec {
        name: "file.has.owner",
        kind: PredicateKind::FileOwnerGate,
    },
    PredicateSpec {
        name: "file.has.contributor",
        kind: PredicateKind::FileContributorGate,
    },
];

pub(crate) const PREDICATE_ALIASES: &[PredicateAliasSpec] = &[
    PredicateAliasSpec {
        alias: "repo.has.path",
        canonical: "repo.has.file",
        rewrite: PredicateAliasRewrite::RepoHasPathScalarToPathFilter,
    },
    // Sourcegraph documents `repo:contains.file(...)` as a pure alias of
    // `repo:has.file(...)`, so it forwards the full matcher surface (scalar path
    // shorthand and `path:` / `name:` / `lang:` filters) unchanged.
    PredicateAliasSpec {
        alias: "repo.contains.file",
        canonical: "repo.has.file",
        rewrite: PredicateAliasRewrite::IdentityArgs,
    },
    // `repo:contains.path(...)` is Sourcegraph's alias of `repo:has.path(...)`,
    // itself an alias of `repo:has.file(path:...)`. Single-level canonicalization
    // collapses it directly onto `repo.has.file`, mirroring the `repo.has.path`
    // scalar→`path:` rewrite so `kind_of` resolves a registry kind.
    PredicateAliasSpec {
        alias: "repo.contains.path",
        canonical: "repo.has.file",
        rewrite: PredicateAliasRewrite::RepoHasPathScalarToPathFilter,
    },
    PredicateAliasSpec {
        alias: "file.contains.content",
        canonical: "file.contains",
        rewrite: PredicateAliasRewrite::IdentityArgs,
    },
    PredicateAliasSpec {
        alias: "repo.contains.content",
        canonical: "repo.has.content",
        rewrite: PredicateAliasRewrite::IdentityArgs,
    },
    PredicateAliasSpec {
        alias: "repo.contains.commit.after",
        canonical: "repo.has.commit.after",
        rewrite: PredicateAliasRewrite::IdentityArgs,
    },
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CanonicalPredicateCall {
    pub name: &'static str,
    pub args: Vec<LqPredicateArg>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PredicateCanonicalizeError {
    InvalidAliasShape,
}

fn registry_spec(name: &str) -> Option<&'static PredicateSpec> {
    PREDICATE_REGISTRY.iter().find(|spec| spec.name == name)
}

fn alias_spec(name: &str) -> Option<&'static PredicateAliasSpec> {
    PREDICATE_ALIASES.iter().find(|spec| spec.alias == name)
}

pub(crate) fn canonical_predicate_name(name: &str) -> Option<&'static str> {
    registry_spec(name)
        .map(|spec| spec.name)
        .or_else(|| alias_spec(name).map(|spec| spec.canonical))
}

pub(crate) fn canonicalize_predicate_call(
    name: &str,
    args: &[LqPredicateArg],
) -> Result<Option<CanonicalPredicateCall>, PredicateCanonicalizeError> {
    if let Some(spec) = registry_spec(name) {
        return Ok(Some(CanonicalPredicateCall {
            name: spec.name,
            args: args.to_vec(),
        }));
    }
    let Some(alias) = alias_spec(name) else {
        return Ok(None);
    };
    let args = match alias.rewrite {
        PredicateAliasRewrite::IdentityArgs => args.to_vec(),
        PredicateAliasRewrite::RepoHasPathScalarToPathFilter => {
            let pattern = match args {
                [LqPredicateArg::Keyword(value)]
                | [LqPredicateArg::Phrase(value)]
                | [LqPredicateArg::RawString(value)] => value.clone(),
                _ => return Err(PredicateCanonicalizeError::InvalidAliasShape),
            };
            vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: pattern,
            }]
        }
    };
    Ok(Some(CanonicalPredicateCall {
        name: alias.canonical,
        args,
    }))
}

/// Resolve a predicate name to its lowering [`PredicateKind`], or `None` when
/// the name is outside the executable lexical subset.
pub(crate) fn kind_of(name: &str) -> Option<PredicateKind> {
    canonical_predicate_name(name)
        .and_then(registry_spec)
        .map(|spec| spec.kind)
}

/// Build the canonical typed reject for a predicate shape the registry does not
/// admit. Centralizes the wire code so every lowering site stays consistent.
pub(crate) fn unimplemented_predicate(message: String) -> CoreError {
    CoreError::Typed {
        code: PREDICATE_UNIMPLEMENTED_CODE.to_string(),
        message,
    }
}

/// A single resolved `repo.has.file` matcher.
///
/// Each matcher narrows the repo-existence gate on a distinct indexed field:
/// `Path` against the full repo-relative path, `Name` against the file name,
/// `Language` against the indexed language code (ADV-01 widened arg-shape
/// family). Each lowers to exactly one canonical field query in `lib.rs`, so
/// the widening adds no ambiguity.
#[derive(Clone)]
pub(crate) enum RepoFileMatcher {
    Path(String),
    Name(String),
    Language(String),
}

/// A fully validated `repo.has.file` argument set.
pub(crate) struct RepoFileConstraint {
    pub matchers: Vec<RepoFileMatcher>,
}

/// Why a `repo.has.file` argument set is not admissible.
///
/// Kept route-neutral so both the lexical lowering (which maps it to a
/// [`CoreError`]) and the planner (which maps it to its own typed error) can
/// reuse one validator without coupling to a single error type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepoFileArgError {
    /// The argument set was not one admitted shape:
    /// - exactly one textual scalar path shorthand, or
    /// - one or more `path:` / `name:` / `lang:` filters.
    UnsupportedArg,
    /// No `path:` / `name:` / `lang:` matcher was supplied.
    NoMatcher,
    /// A `lang:` matcher value was empty or whitespace-only.
    EmptyLanguageValue,
}

/// Parse `repo.has.file(...)` arguments into validated matchers.
///
/// This is the single owner of the `repo.has.file` argument contract:
///
/// - exactly one textual scalar path shorthand (`repo.has.file(src/lib.rs)`), or
/// - one or more `path:` / `name:` / `lang:` filters
///
/// are admitted, and at least one matcher is required. Both lexical lowering
/// and planner validation route through here so the contract cannot diverge
/// between them.
pub(crate) fn parse_repo_file_matchers(
    args: &[LqPredicateArg],
) -> Result<RepoFileConstraint, RepoFileArgError> {
    match args {
        [LqPredicateArg::Keyword(value)]
        | [LqPredicateArg::Phrase(value)]
        | [LqPredicateArg::RawString(value)] => {
            return Ok(RepoFileConstraint {
                matchers: vec![RepoFileMatcher::Path(value.clone())],
            });
        }
        [LqPredicateArg::Number(_)] => return Err(RepoFileArgError::UnsupportedArg),
        _ => {}
    }
    let mut matchers: Vec<RepoFileMatcher> = Vec::new();
    for arg in args {
        match arg {
            LqPredicateArg::Filter { name, value } if name == "path" => {
                matchers.push(RepoFileMatcher::Path(value.clone()));
            }
            LqPredicateArg::Filter { name, value } if name == "name" => {
                matchers.push(RepoFileMatcher::Name(value.clone()));
            }
            LqPredicateArg::Filter { name, value } if name == "lang" => {
                // Registry owns value validity too: an empty/whitespace lang is
                // not a usable matcher, so reject it here rather than deferring
                // to the executor.
                if value.trim().is_empty() {
                    return Err(RepoFileArgError::EmptyLanguageValue);
                }
                matchers.push(RepoFileMatcher::Language(value.clone()));
            }
            LqPredicateArg::Keyword(_)
            | LqPredicateArg::Phrase(_)
            | LqPredicateArg::RawString(_)
            | LqPredicateArg::Number(_)
            | LqPredicateArg::Filter { .. } => {
                return Err(RepoFileArgError::UnsupportedArg);
            }
        }
    }
    if matchers.is_empty() {
        return Err(RepoFileArgError::NoMatcher);
    }
    Ok(RepoFileConstraint { matchers })
}

/// Shared scalar contract for executable lexical content predicates.
///
/// `file.contains`, `file.has.content`, and `repo.has.content` all lower
/// through the same lexical content substrate. The shipped widening admits a
/// single keyword / phrase / raw-string / number scalar; `Number` is
/// canonicalized downstream to `Keyword(<decimal>)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContentScalarArg {
    Keyword(String),
    Phrase(String),
    RawString(String),
    Number(i64),
}

/// Why a shared content-scalar family is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentScalarArgError {
    /// Not exactly one positional argument.
    WrongArity,
    /// The single argument was not an admitted scalar — e.g. a `filter:` arg.
    UnsupportedArg,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimerefScalarArg {
    pub value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimerefScalarArgError {
    WrongArity,
    UnsupportedArg,
}

pub(crate) fn parse_timeref_scalar_arg(
    args: &[LqPredicateArg],
) -> Result<TimerefScalarArg, TimerefScalarArgError> {
    let [arg] = args else {
        return Err(TimerefScalarArgError::WrongArity);
    };
    match arg {
        LqPredicateArg::Keyword(value)
        | LqPredicateArg::Phrase(value)
        | LqPredicateArg::RawString(value) => Ok(TimerefScalarArg {
            value: value.clone(),
        }),
        LqPredicateArg::Number(value) => Ok(TimerefScalarArg {
            value: value.to_string(),
        }),
        LqPredicateArg::Filter { .. } => Err(TimerefScalarArgError::UnsupportedArg),
    }
}

/// A validated `repo.has.meta(key:value)` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepoMetaArg {
    pub key: String,
    pub value: String,
}

/// A validated `repo.has.topic(topic)` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepoTopicArg {
    pub topic: String,
}

/// A validated `file.has.owner` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileOwnerArg {
    pub owner: Option<String>,
}

/// A validated `file.has.contributor` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileContributorArg {
    pub contributor: String,
}

/// Why a `file.has.owner` argument set is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileOwnerArgError {
    WrongArity,
    UnsupportedArg,
    EmptyOwner,
}

/// Why a `file.has.contributor` argument set is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileContributorArgError {
    WrongArity,
    UnsupportedArg,
    EmptyContributor,
    /// The contributor identity was wrapped in `/.../ ` regex delimiters
    /// (`file:has.contributor(/name/)`). The contributor authority is an
    /// exact-string set with no name/email split and no regex engine, so regex
    /// contributor matching is unsupported.
    RegexUnsupported,
}

/// Why a `repo.has.topic` argument set is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepoTopicArgError {
    WrongArity,
    UnsupportedArg,
    EmptyTopic,
}

/// Why a `repo.has.meta` argument set is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepoMetaArgError {
    /// Not exactly one positional argument.
    WrongArity,
    /// The single argument was not a `key:value` filter (e.g. key-only or a
    /// bare scalar). Key-only shapes stay typed-fail: the exact-string metadata
    /// substrate exposes only `key == value` equality, not key existence.
    UnsupportedArg,
    /// The `key:value` filter carried an empty/whitespace-only key.
    EmptyKey,
    /// The `key:value` filter carried an empty/whitespace-only value
    /// (`repo:has.meta(tag:)`). The exact-string substrate has no tag/null-value
    /// concept, so an empty value is not an admitted shape.
    EmptyValue,
    /// The key or value was wrapped in `/.../ ` regex delimiters
    /// (`repo:has.meta(/key/:/value/)`). The metadata authority is exact-string
    /// only and carries no regex engine, so regex key/value is unsupported.
    RegexUnsupported,
}

/// Whether a token is wrapped in `/.../ ` regex delimiters (a leading AND
/// trailing slash). A token that merely contains a slash (e.g. a path-like
/// `/usr/bin`, or a single leading `/x`) is not delimited and keeps exact-string
/// semantics.
///
/// Trade-off: a slash-terminated value such as `/usr/` is treated as regex
/// syntax and fails closed, even though it could be an exact path. This is the
/// deliberate fail-closed choice — on an exact-string substrate we refuse
/// ambiguous regex-shaped input rather than silently matching the literal
/// slashes, which would misrepresent regex support. Metadata keys/values and
/// contributor identities are not slash-wrapped in practice.
fn is_regex_delimited(token: &str) -> bool {
    let token = token.trim();
    token.len() >= 2 && token.starts_with('/') && token.ends_with('/')
}

/// Parse a `repo.has.meta(key:value)` argument.
///
/// Single owner of the `repo.has.meta` argument contract: exactly one
/// `key:value` filter is admitted, surfaced by the parser as
/// [`LqPredicateArg::Filter`]. Both lexical lowering and planner validation
/// route through here so the contract cannot diverge.
pub(crate) fn parse_repo_meta_arg(
    args: &[LqPredicateArg],
) -> Result<RepoMetaArg, RepoMetaArgError> {
    let [arg] = args else {
        return Err(RepoMetaArgError::WrongArity);
    };
    match arg {
        LqPredicateArg::Filter { name, value } => {
            if name.trim().is_empty() {
                return Err(RepoMetaArgError::EmptyKey);
            }
            // `/key/:/value/` regex syntax has no regex-capable authority on the
            // exact-string substrate; fail closed rather than literal-matching
            // the slashes. Checked before the empty-value guard so a regex key
            // with an empty value still reports the regex reason.
            if is_regex_delimited(name) || is_regex_delimited(value) {
                return Err(RepoMetaArgError::RegexUnsupported);
            }
            if value.trim().is_empty() {
                return Err(RepoMetaArgError::EmptyValue);
            }
            Ok(RepoMetaArg {
                key: name.clone(),
                value: value.clone(),
            })
        }
        LqPredicateArg::Keyword(_)
        | LqPredicateArg::Phrase(_)
        | LqPredicateArg::RawString(_)
        | LqPredicateArg::Number(_) => Err(RepoMetaArgError::UnsupportedArg),
    }
}

/// Parse a `repo.has.topic(topic)` argument.
pub(crate) fn parse_repo_topic_arg(
    args: &[LqPredicateArg],
) -> Result<RepoTopicArg, RepoTopicArgError> {
    let [arg] = args else {
        return Err(RepoTopicArgError::WrongArity);
    };
    match arg {
        LqPredicateArg::Keyword(value)
        | LqPredicateArg::Phrase(value)
        | LqPredicateArg::RawString(value) => {
            if value.trim().is_empty() {
                return Err(RepoTopicArgError::EmptyTopic);
            }
            Ok(RepoTopicArg {
                topic: value.clone(),
            })
        }
        LqPredicateArg::Number(_) | LqPredicateArg::Filter { .. } => {
            Err(RepoTopicArgError::UnsupportedArg)
        }
    }
}

/// Parse a `file.has.owner` argument.
///
/// Admitted shapes:
/// - `file.has.owner()` => any owner
/// - `file.has.owner(<keyword|phrase|raw>)` => exact owner identity
pub(crate) fn parse_file_owner_arg(
    args: &[LqPredicateArg],
) -> Result<FileOwnerArg, FileOwnerArgError> {
    match args {
        [] => Ok(FileOwnerArg { owner: None }),
        [LqPredicateArg::Keyword(value)]
        | [LqPredicateArg::Phrase(value)]
        | [LqPredicateArg::RawString(value)] => {
            if value.trim().is_empty() {
                return Err(FileOwnerArgError::EmptyOwner);
            }
            Ok(FileOwnerArg {
                owner: Some(value.clone()),
            })
        }
        [LqPredicateArg::Number(_)] | [LqPredicateArg::Filter { .. }] => {
            Err(FileOwnerArgError::UnsupportedArg)
        }
        [_, _, ..] => Err(FileOwnerArgError::WrongArity),
    }
}

/// Parse a `file.has.contributor` argument.
///
/// Admitted shapes:
/// - `file.has.contributor(<keyword|phrase|raw>)` => exact contributor identity
pub(crate) fn parse_file_contributor_arg(
    args: &[LqPredicateArg],
) -> Result<FileContributorArg, FileContributorArgError> {
    let [arg] = args else {
        return Err(FileContributorArgError::WrongArity);
    };
    match arg {
        LqPredicateArg::Keyword(value)
        | LqPredicateArg::Phrase(value)
        | LqPredicateArg::RawString(value) => {
            if value.trim().is_empty() {
                return Err(FileContributorArgError::EmptyContributor);
            }
            // `/name/` regex syntax has no regex-capable contributor authority;
            // fail closed rather than literal-matching the slashes.
            if is_regex_delimited(value) {
                return Err(FileContributorArgError::RegexUnsupported);
            }
            Ok(FileContributorArg {
                contributor: value.clone(),
            })
        }
        LqPredicateArg::Number(_) | LqPredicateArg::Filter { .. } => {
            Err(FileContributorArgError::UnsupportedArg)
        }
    }
}

/// Parse a one-scalar content predicate argument.
///
/// Single owner of the shared content scalar contract. Both lexical lowering
/// and planner validation route through here so the contract cannot diverge.
pub(crate) fn parse_content_scalar_arg(
    args: &[LqPredicateArg],
) -> Result<ContentScalarArg, ContentScalarArgError> {
    let [arg] = args else {
        return Err(ContentScalarArgError::WrongArity);
    };
    match arg {
        LqPredicateArg::Keyword(value) => Ok(ContentScalarArg::Keyword(value.clone())),
        LqPredicateArg::Phrase(value) => Ok(ContentScalarArg::Phrase(value.clone())),
        LqPredicateArg::RawString(value) => Ok(ContentScalarArg::RawString(value.clone())),
        LqPredicateArg::Number(value) => Ok(ContentScalarArg::Number(*value)),
        LqPredicateArg::Filter { .. } => Err(ContentScalarArgError::UnsupportedArg),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContentPathScope {
    pub pattern: String,
    pub scope: LqFileScope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContentPredicateConstraint {
    pub content: ContentScalarArg,
    pub path_scope: Option<ContentPathScope>,
    pub language: Option<String>,
}

impl ContentPredicateConstraint {
    pub(crate) const fn has_scopes(&self) -> bool {
        self.path_scope.is_some() || self.language.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentPredicateArgError {
    MissingContentScalar,
    MultipleContentScalars,
    UnsupportedFilter,
    DuplicatePathScope,
    DuplicateLanguage,
    EmptyLanguageValue,
}

pub(crate) fn parse_content_predicate_constraint(
    args: &[LqPredicateArg],
) -> Result<ContentPredicateConstraint, ContentPredicateArgError> {
    let mut content: Option<ContentScalarArg> = None;
    let mut path_scope: Option<ContentPathScope> = None;
    let mut language: Option<String> = None;
    for arg in args {
        match arg {
            LqPredicateArg::Keyword(value) => {
                if content
                    .replace(ContentScalarArg::Keyword(value.clone()))
                    .is_some()
                {
                    return Err(ContentPredicateArgError::MultipleContentScalars);
                }
            }
            LqPredicateArg::Phrase(value) => {
                if content
                    .replace(ContentScalarArg::Phrase(value.clone()))
                    .is_some()
                {
                    return Err(ContentPredicateArgError::MultipleContentScalars);
                }
            }
            LqPredicateArg::RawString(value) => {
                if content
                    .replace(ContentScalarArg::RawString(value.clone()))
                    .is_some()
                {
                    return Err(ContentPredicateArgError::MultipleContentScalars);
                }
            }
            LqPredicateArg::Number(value) => {
                if content.replace(ContentScalarArg::Number(*value)).is_some() {
                    return Err(ContentPredicateArgError::MultipleContentScalars);
                }
            }
            LqPredicateArg::Filter { name, value } if name == "file" => {
                if path_scope.is_some() {
                    return Err(ContentPredicateArgError::DuplicatePathScope);
                }
                path_scope = Some(ContentPathScope {
                    pattern: value.clone(),
                    scope: LqFileScope::NameAndPath,
                });
            }
            LqPredicateArg::Filter { name, value } if name == "path" => {
                if path_scope.is_some() {
                    return Err(ContentPredicateArgError::DuplicatePathScope);
                }
                path_scope = Some(ContentPathScope {
                    pattern: value.clone(),
                    scope: LqFileScope::PathOnly,
                });
            }
            LqPredicateArg::Filter { name, value } if name == "lang" => {
                if value.trim().is_empty() {
                    return Err(ContentPredicateArgError::EmptyLanguageValue);
                }
                if language.replace(value.clone()).is_some() {
                    return Err(ContentPredicateArgError::DuplicateLanguage);
                }
            }
            LqPredicateArg::Filter { .. } => {
                return Err(ContentPredicateArgError::UnsupportedFilter);
            }
        }
    }
    let Some(content) = content else {
        return Err(ContentPredicateArgError::MissingContentScalar);
    };
    Ok(ContentPredicateConstraint {
        content,
        path_scope,
        language,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_resolves_shipped_predicate_kinds() {
        assert_eq!(kind_of("file.contains"), Some(PredicateKind::ContentLeaf));
        assert_eq!(
            kind_of("file.has.content"),
            Some(PredicateKind::ContentLeaf)
        );
        assert_eq!(kind_of("repo.has.file"), Some(PredicateKind::RepoFileGate));
        assert_eq!(
            kind_of("repo.has.commit.after"),
            Some(PredicateKind::RepoCommitRecencyGate)
        );
        assert_eq!(kind_of("repo.has.meta"), Some(PredicateKind::RepoMetaGate));
        assert_eq!(
            kind_of("repo.has.topic"),
            Some(PredicateKind::RepoTopicGate)
        );
        assert_eq!(
            kind_of("file.has.owner"),
            Some(PredicateKind::FileOwnerGate)
        );
        assert_eq!(
            kind_of("file.has.contributor"),
            Some(PredicateKind::FileContributorGate)
        );
    }

    #[test]
    fn repo_meta_arg_admits_key_value_and_rejects_other_shapes() {
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "license".to_string(),
                value: "MIT".to_string(),
            }]),
            Ok(RepoMetaArg {
                key: "license".to_string(),
                value: "MIT".to_string(),
            })
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Keyword("license".to_string())]),
            Err(RepoMetaArgError::UnsupportedArg)
        );
        assert_eq!(parse_repo_meta_arg(&[]), Err(RepoMetaArgError::WrongArity));
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "  ".to_string(),
                value: "MIT".to_string(),
            }]),
            Err(RepoMetaArgError::EmptyKey)
        );
    }

    #[test]
    fn repo_meta_arg_rejects_empty_value_tag_shape() {
        // SGT-03: `repo:has.meta(tag:)` parses to a `tag` key with an empty
        // value. The exact-string substrate has no tag/null-value concept, so
        // an empty value must fail closed instead of silently exact-matching the
        // empty string — that would overstate tag-null support.
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "tag".to_string(),
                value: "".to_string(),
            }]),
            Err(RepoMetaArgError::EmptyValue)
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "license".to_string(),
                value: "   ".to_string(),
            }]),
            Err(RepoMetaArgError::EmptyValue)
        );
    }

    #[test]
    fn repo_meta_arg_rejects_slash_delimited_regex_shape() {
        // SGT-03: `repo:has.meta(/key/:/value/)` is Sourcegraph regex key/value
        // syntax. The metadata authority is an exact-string `BTreeMap` with no
        // regex engine, so a `/.../`-delimited key or value must fail closed
        // rather than silently exact-match the literal slashes.
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "/key/".to_string(),
                value: "/value/".to_string(),
            }]),
            Err(RepoMetaArgError::RegexUnsupported)
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "license".to_string(),
                value: "/apache.*/".to_string(),
            }]),
            Err(RepoMetaArgError::RegexUnsupported)
        );
        // A regex KEY with a plain value is rejected too — the guard checks both
        // operands, not just the value.
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "/key/".to_string(),
                value: "plain".to_string(),
            }]),
            Err(RepoMetaArgError::RegexUnsupported)
        );
        // A value that merely contains a slash (path-like) is NOT regex syntax
        // and must keep exact-string semantics.
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "/usr/bin".to_string(),
            }]),
            Ok(RepoMetaArg {
                key: "path".to_string(),
                value: "/usr/bin".to_string(),
            })
        );
    }

    #[test]
    fn repo_topic_arg_accepts_one_textual_topic() {
        assert_eq!(
            parse_repo_topic_arg(&[LqPredicateArg::Keyword("security".to_string())]),
            Ok(RepoTopicArg {
                topic: "security".to_string(),
            })
        );
        assert_eq!(
            parse_repo_topic_arg(&[LqPredicateArg::Phrase("ml-platform".to_string())]),
            Ok(RepoTopicArg {
                topic: "ml-platform".to_string(),
            })
        );
    }

    #[test]
    fn repo_topic_arg_rejects_bad_arity_and_non_textual_shapes() {
        assert_eq!(
            parse_repo_topic_arg(&[]),
            Err(RepoTopicArgError::WrongArity)
        );
        assert_eq!(
            parse_repo_topic_arg(&[
                LqPredicateArg::Keyword("security".to_string()),
                LqPredicateArg::Keyword("ml".to_string()),
            ]),
            Err(RepoTopicArgError::WrongArity)
        );
        assert_eq!(
            parse_repo_topic_arg(&[LqPredicateArg::Number(7)]),
            Err(RepoTopicArgError::UnsupportedArg)
        );
        assert_eq!(
            parse_repo_topic_arg(&[LqPredicateArg::Filter {
                name: "topic".to_string(),
                value: "security".to_string(),
            }]),
            Err(RepoTopicArgError::UnsupportedArg)
        );
        assert_eq!(
            parse_repo_topic_arg(&[LqPredicateArg::Keyword(" ".to_string())]),
            Err(RepoTopicArgError::EmptyTopic)
        );
    }

    #[test]
    fn registry_rejects_names_outside_subset() {
        // `symbol.has.name` is a symbol-route predicate, not a registry member.
        assert_eq!(kind_of("symbol.has.name"), None);
        assert_eq!(kind_of("repo.has.commit"), None);
        assert_eq!(kind_of(""), None);
    }

    #[test]
    fn registry_resolves_native_aliases_to_canonical_kinds() {
        assert_eq!(kind_of("repo.has.path"), Some(PredicateKind::RepoFileGate));
        assert_eq!(
            kind_of("file.contains.content"),
            Some(PredicateKind::ContentLeaf)
        );
        assert_eq!(
            kind_of("repo.contains.content"),
            Some(PredicateKind::RepoContentGate)
        );
        assert_eq!(
            kind_of("repo.contains.commit.after"),
            Some(PredicateKind::RepoCommitRecencyGate)
        );
    }

    #[test]
    fn canonicalize_repo_has_path_alias_rewrites_to_path_filter() {
        let call = canonicalize_predicate_call(
            "repo.has.path",
            &[LqPredicateArg::Keyword("src/lib.rs".to_string())],
        )
        .expect("alias canonicalization must succeed")
        .expect("alias must resolve");
        assert_eq!(call.name, "repo.has.file");
        assert_eq!(
            call.args,
            vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            }]
        );
    }

    #[test]
    fn canonicalize_repo_has_path_alias_rejects_non_scalar_shape() {
        assert_eq!(
            canonicalize_predicate_call(
                "repo.has.path",
                &[LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/lib.rs".to_string(),
                }]
            ),
            Err(PredicateCanonicalizeError::InvalidAliasShape)
        );
    }

    #[test]
    fn registry_resolves_repo_contains_file_and_path_aliases() {
        // Sourcegraph documents `repo:contains.file(...)` as an alias of
        // `repo:has.file(...)` and `repo:contains.path(...)` as an alias of
        // `repo:has.path(...)`. Both canonicalize onto the executable
        // `RepoFileGate` row.
        assert_eq!(
            kind_of("repo.contains.file"),
            Some(PredicateKind::RepoFileGate)
        );
        assert_eq!(
            kind_of("repo.contains.path"),
            Some(PredicateKind::RepoFileGate)
        );
    }

    #[test]
    fn canonicalize_repo_contains_file_alias_forwards_matcher_args_identity() {
        // `repo.contains.file` mirrors the full `repo.has.file` matcher surface
        // (scalar path shorthand AND path:/name:/lang: filters), so it forwards
        // args identically — unlike `repo.has.path`, it does NOT collapse to a
        // path filter.
        let call = canonicalize_predicate_call(
            "repo.contains.file",
            &[LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "lib.rs".to_string(),
            }],
        )
        .expect("alias canonicalization must succeed")
        .expect("alias must resolve");
        assert_eq!(call.name, "repo.has.file");
        assert_eq!(
            call.args,
            vec![LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "lib.rs".to_string(),
            }]
        );

        // Scalar shorthand forwards as-is; the RepoFileGate executor treats a
        // bare scalar as a path shorthand.
        let scalar = canonicalize_predicate_call(
            "repo.contains.file",
            &[LqPredicateArg::Keyword("src/lib.rs".to_string())],
        )
        .expect("alias canonicalization must succeed")
        .expect("alias must resolve");
        assert_eq!(scalar.name, "repo.has.file");
        assert_eq!(
            scalar.args,
            vec![LqPredicateArg::Keyword("src/lib.rs".to_string())]
        );
    }

    #[test]
    fn canonicalize_repo_contains_path_alias_rewrites_to_path_filter() {
        // `repo.contains.path` is the alias-of-alias of `repo.has.path`; it
        // collapses directly to `repo.has.file(path:...)` so single-level
        // canonicalization resolves a registry kind.
        let call = canonicalize_predicate_call(
            "repo.contains.path",
            &[LqPredicateArg::Keyword("src/lib.rs".to_string())],
        )
        .expect("alias canonicalization must succeed")
        .expect("alias must resolve");
        assert_eq!(call.name, "repo.has.file");
        assert_eq!(
            call.args,
            vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            }]
        );
    }

    #[test]
    fn canonicalize_repo_contains_path_alias_rejects_non_scalar_shape() {
        // The scalar→path rewrite rejects every non-scalar shape, not just a
        // `path:` filter: number, empty, and multi-arg all fail closed.
        for args in [
            vec![LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            }],
            vec![LqPredicateArg::Number(7)],
            vec![],
            vec![
                LqPredicateArg::Keyword("a".to_string()),
                LqPredicateArg::Keyword("b".to_string()),
            ],
        ] {
            assert_eq!(
                canonicalize_predicate_call("repo.contains.path", &args),
                Err(PredicateCanonicalizeError::InvalidAliasShape),
                "repo.contains.path must reject non-scalar shape {args:?}",
            );
        }
    }

    #[test]
    fn repo_file_matchers_reject_content_filter() {
        // SGT-01 Cell 3: `repo:has.file(path:... content:...)` has no executable
        // owner seam on the repo-file gate (no Content matcher variant). The
        // nested `content:` filter must fail closed, not be silently widened.
        assert!(matches!(
            parse_repo_file_matchers(&[
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/lib.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "content".to_string(),
                    value: "needle".to_string(),
                },
            ]),
            Err(RepoFileArgError::UnsupportedArg)
        ));
    }

    #[test]
    fn registry_resolves_repo_has_content_kind() {
        assert_eq!(
            kind_of("repo.has.content"),
            Some(PredicateKind::RepoContentGate)
        );
    }

    #[test]
    fn file_owner_arg_accepts_zero_or_one_textual_owner() {
        assert_eq!(parse_file_owner_arg(&[]), Ok(FileOwnerArg { owner: None }));
        assert_eq!(
            parse_file_owner_arg(&[LqPredicateArg::Keyword("@alice".to_string())]),
            Ok(FileOwnerArg {
                owner: Some("@alice".to_string()),
            })
        );
        assert_eq!(
            parse_file_owner_arg(&[LqPredicateArg::Phrase("@acme/platform".to_string())]),
            Ok(FileOwnerArg {
                owner: Some("@acme/platform".to_string()),
            })
        );
    }

    #[test]
    fn file_owner_arg_rejects_bad_arity_and_non_textual_shapes() {
        assert_eq!(
            parse_file_owner_arg(&[
                LqPredicateArg::Keyword("@alice".to_string()),
                LqPredicateArg::Keyword("@bob".to_string()),
            ]),
            Err(FileOwnerArgError::WrongArity)
        );
        assert_eq!(
            parse_file_owner_arg(&[LqPredicateArg::Number(7)]),
            Err(FileOwnerArgError::UnsupportedArg)
        );
        assert_eq!(
            parse_file_owner_arg(&[LqPredicateArg::Filter {
                name: "owner".to_string(),
                value: "@alice".to_string(),
            }]),
            Err(FileOwnerArgError::UnsupportedArg)
        );
        assert_eq!(
            parse_file_owner_arg(&[LqPredicateArg::Keyword(" ".to_string())]),
            Err(FileOwnerArgError::EmptyOwner)
        );
    }

    #[test]
    fn file_contributor_arg_accepts_one_textual_contributor() {
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword("alice".to_string())]),
            Ok(FileContributorArg {
                contributor: "alice".to_string(),
            })
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Phrase("carol@example.com".to_string())]),
            Ok(FileContributorArg {
                contributor: "carol@example.com".to_string(),
            })
        );
    }

    #[test]
    fn file_contributor_arg_rejects_bad_arity_and_non_textual_shapes() {
        assert_eq!(
            parse_file_contributor_arg(&[]),
            Err(FileContributorArgError::WrongArity)
        );
        assert_eq!(
            parse_file_contributor_arg(&[
                LqPredicateArg::Keyword("alice".to_string()),
                LqPredicateArg::Keyword("bob".to_string()),
            ]),
            Err(FileContributorArgError::WrongArity)
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Number(7)]),
            Err(FileContributorArgError::UnsupportedArg)
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Filter {
                name: "contributor".to_string(),
                value: "alice".to_string(),
            }]),
            Err(FileContributorArgError::UnsupportedArg)
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword(" ".to_string())]),
            Err(FileContributorArgError::EmptyContributor)
        );
    }

    #[test]
    fn file_contributor_arg_rejects_slash_delimited_regex_shape() {
        // SGT-04: Sourcegraph documents `file:has.contributor(<regex>)` as a
        // name-or-email regex. This stack stores a flat, case-folded exact-string
        // contributor set with no name/email split and no regex engine, so a
        // `/.../ `-delimited arg must fail closed rather than silently exact-match
        // the literal slashes to nothing (regex theater over exact strings).
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword("/ali.*/".to_string())]),
            Err(FileContributorArgError::RegexUnsupported)
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Phrase("/carol@.*/".to_string())]),
            Err(FileContributorArgError::RegexUnsupported)
        );
        // An exact identity that merely contains a slash is not regex syntax and
        // keeps exact-string semantics.
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword("a/b".to_string())]),
            Ok(FileContributorArg {
                contributor: "a/b".to_string(),
            })
        );
        // Boundary: a single leading slash is NOT a `/.../ ` delimiter pair, so
        // it stays an exact identity rather than being misread as regex.
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword("/x".to_string())]),
            Ok(FileContributorArg {
                contributor: "/x".to_string(),
            })
        );
    }

    #[test]
    fn content_scalar_arg_accepts_textual_and_numeric_scalars() {
        for arg in [
            LqPredicateArg::Keyword("needle".to_string()),
            LqPredicateArg::Phrase("two words".to_string()),
            LqPredicateArg::RawString("raw".to_string()),
            LqPredicateArg::Number(7),
        ] {
            assert!(parse_content_scalar_arg(&[arg]).is_ok());
        }
    }

    #[test]
    fn content_scalar_arg_rejects_filter_and_bad_arity() {
        assert_eq!(
            parse_content_scalar_arg(&[]),
            Err(ContentScalarArgError::WrongArity)
        );
        assert_eq!(
            parse_content_scalar_arg(&[
                LqPredicateArg::Keyword("a".to_string()),
                LqPredicateArg::Keyword("b".to_string()),
            ]),
            Err(ContentScalarArgError::WrongArity)
        );
        assert_eq!(
            parse_content_scalar_arg(&[LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "x".to_string(),
            }]),
            Err(ContentScalarArgError::UnsupportedArg)
        );
    }

    #[test]
    fn content_predicate_constraint_accepts_scope_filters_and_number_scalar() {
        let constraint = parse_content_predicate_constraint(&[
            LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src".to_string(),
            },
            LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: "rust".to_string(),
            },
            LqPredicateArg::Number(123),
        ])
        .expect("scoped number content predicate accepted");
        assert_eq!(constraint.content, ContentScalarArg::Number(123));
        assert_eq!(
            constraint.path_scope,
            Some(ContentPathScope {
                pattern: "src".to_string(),
                scope: LqFileScope::PathOnly,
            })
        );
        assert_eq!(constraint.language.as_deref(), Some("rust"));
    }

    #[test]
    fn content_predicate_constraint_rejects_missing_duplicate_and_unknown_shapes() {
        assert_eq!(
            parse_content_predicate_constraint(&[]),
            Err(ContentPredicateArgError::MissingContentScalar)
        );
        assert_eq!(
            parse_content_predicate_constraint(&[
                LqPredicateArg::Keyword("a".to_string()),
                LqPredicateArg::Phrase("b".to_string()),
            ]),
            Err(ContentPredicateArgError::MultipleContentScalars)
        );
        assert_eq!(
            parse_content_predicate_constraint(&[
                LqPredicateArg::Filter {
                    name: "repo".to_string(),
                    value: "corp-a".to_string(),
                },
                LqPredicateArg::Keyword("a".to_string()),
            ]),
            Err(ContentPredicateArgError::UnsupportedFilter)
        );
        assert_eq!(
            parse_content_predicate_constraint(&[
                LqPredicateArg::Filter {
                    name: "file".to_string(),
                    value: "Cargo.toml".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src".to_string(),
                },
                LqPredicateArg::Keyword("a".to_string()),
            ]),
            Err(ContentPredicateArgError::DuplicatePathScope)
        );
        assert_eq!(
            parse_content_predicate_constraint(&[
                LqPredicateArg::Filter {
                    name: "lang".to_string(),
                    value: " ".to_string(),
                },
                LqPredicateArg::Keyword("a".to_string()),
            ]),
            Err(ContentPredicateArgError::EmptyLanguageValue)
        );
    }

    #[test]
    fn repo_file_matchers_accept_path_and_name_filters() {
        let args = vec![
            LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            },
            LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "lib.rs".to_string(),
            },
        ];
        let constraint = parse_repo_file_matchers(&args).expect("path/name accepted");
        assert_eq!(constraint.matchers.len(), 2);
        assert!(matches!(constraint.matchers[0], RepoFileMatcher::Path(_)));
        assert!(matches!(constraint.matchers[1], RepoFileMatcher::Name(_)));
    }

    #[test]
    fn repo_file_matchers_accept_single_scalar_path_shorthand() {
        for arg in [
            LqPredicateArg::Keyword("src/lib.rs".to_string()),
            LqPredicateArg::Phrase("src/lib.rs".to_string()),
            LqPredicateArg::RawString("src/lib.rs".to_string()),
        ] {
            let constraint = parse_repo_file_matchers(&[arg]).expect("scalar path accepted");
            assert_eq!(constraint.matchers.len(), 1);
            assert!(matches!(
                constraint.matchers[0],
                RepoFileMatcher::Path(ref v) if v == "src/lib.rs"
            ));
        }
    }

    #[test]
    fn repo_file_matchers_accept_lang_filter() {
        // ADV-01 widened arg-shape family: `lang:` maps to one canonical matcher.
        let args = vec![LqPredicateArg::Filter {
            name: "lang".to_string(),
            value: "rust".to_string(),
        }];
        let constraint = parse_repo_file_matchers(&args).expect("lang accepted");
        assert_eq!(constraint.matchers.len(), 1);
        assert!(matches!(
            constraint.matchers[0],
            RepoFileMatcher::Language(ref v) if v == "rust"
        ));
    }

    #[test]
    fn repo_file_matchers_reject_empty_lang_value() {
        for value in ["", "   ", "\t"] {
            let args = vec![LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: value.to_string(),
            }];
            assert!(
                matches!(
                    parse_repo_file_matchers(&args),
                    Err(RepoFileArgError::EmptyLanguageValue)
                ),
                "empty/whitespace lang value {value:?} must be rejected at the registry"
            );
        }
    }

    #[test]
    fn repo_file_matchers_reject_non_matcher_arg() {
        let mixed = vec![
            LqPredicateArg::Keyword("src/lib.rs".to_string()),
            LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            },
        ];
        assert!(matches!(
            parse_repo_file_matchers(&mixed),
            Err(RepoFileArgError::UnsupportedArg)
        ));
        let number = vec![LqPredicateArg::Number(7)];
        assert!(matches!(
            parse_repo_file_matchers(&number),
            Err(RepoFileArgError::UnsupportedArg)
        ));
        // A filter outside the path/name/lang contract still typed-fails.
        let unknown_filter = vec![LqPredicateArg::Filter {
            name: "size".to_string(),
            value: "big".to_string(),
        }];
        assert!(matches!(
            parse_repo_file_matchers(&unknown_filter),
            Err(RepoFileArgError::UnsupportedArg)
        ));
    }

    #[test]
    fn repo_file_matchers_reject_empty() {
        assert!(matches!(
            parse_repo_file_matchers(&[]),
            Err(RepoFileArgError::NoMatcher)
        ));
    }
}
