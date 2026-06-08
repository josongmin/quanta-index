//! Predicate capability registry — the single source of truth for which
//! lexical predicate leaves are executable on the Tantivy adapter.
//!
//! It records what argument shapes each predicate admits and which lowering
//! target it resolves to.
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
    /// `repo.has.description` — exactly one textual pattern scalar, lowered to a
    /// repo-existence gate evaluated by compiling the pattern as a regex and
    /// matching it against source-repo keyed repo-description authority
    /// materialized alongside the lexical generation.
    RepoDescriptionGate,
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
        name: "repo.has.description",
        kind: PredicateKind::RepoDescriptionGate,
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
                [
                    LqPredicateArg::Keyword(value)
                    | LqPredicateArg::Phrase(value)
                    | LqPredicateArg::RawString(value),
                ] => value.clone(),
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
/// family), `Content` against the chunk text (SGX-01 `path + content`
/// correlation). Each lowers to exactly one canonical field query in `lib.rs`
/// and is AND-ed (`Occur::Must`) per indexed document, so a repo matches only
/// when ONE file satisfies all matchers — no cross-file overmatching.
#[derive(Clone)]
pub(crate) enum RepoFileMatcher {
    Path(String),
    Name(String),
    Language(String),
    /// `content:` chunk-text matcher. Lowered (in `lib.rs`) through the same
    /// `content_leaf_from_scalar` regex-delimiter path the `ContentLeaf` family
    /// uses, so a `/regex/` content stays a regex and a bare value is a standard
    /// tokenized match.
    Content(String),
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
    /// No `path:` / `name:` / `lang:` / `content:` matcher was supplied.
    NoMatcher,
    /// A `lang:` matcher value was empty or whitespace-only.
    EmptyLanguageValue,
    /// A `content:` matcher value was empty or whitespace-only.
    EmptyContentValue,
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
        [
            LqPredicateArg::Keyword(value)
            | LqPredicateArg::Phrase(value)
            | LqPredicateArg::RawString(value),
        ] => {
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
            LqPredicateArg::Filter { name, value } if name == "content" => {
                // SGX-01: `content:` correlates per-document with the path/name
                // matchers. An empty content value is not a usable matcher.
                if value.trim().is_empty() {
                    return Err(RepoFileArgError::EmptyContentValue);
                }
                matchers.push(RepoFileMatcher::Content(value.clone()));
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

/// A validated exact-or-regex metadata matcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MetaPattern {
    Exact(String),
    Regex(String),
}

/// A validated `repo.has.meta(...)` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepoMetaArg {
    pub key: MetaPattern,
    pub value: Option<MetaPattern>,
}

/// A validated `repo.has.topic(topic)` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepoTopicArg {
    pub topic: String,
}

/// A validated `repo.has.description(pattern)` argument.
///
/// The pattern is the verbatim regex source supplied by the caller; it is
/// compiled and matched against the repo description authority at execution
/// time, not here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RepoDescriptionArg {
    pub pattern: String,
}

/// A validated `file.has.owner` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileOwnerArg {
    pub owner: Option<String>,
}

/// A validated `file.has.contributor` argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileContributorArg {
    pub contributor: ContributorPattern,
}

/// The `file.has.contributor` textual pattern shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContributorPattern {
    Exact(String),
    Regex(String),
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
}

/// Why a `repo.has.topic` argument set is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepoTopicArgError {
    WrongArity,
    UnsupportedArg,
    EmptyTopic,
}

/// Why a `repo.has.description` argument set is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepoDescriptionArgError {
    /// Not exactly one positional argument.
    WrongArity,
    /// The single argument was not a keyword/phrase/raw-string pattern (e.g. a
    /// numeric or `key:value` filter arg).
    UnsupportedArg,
    /// The argument carried an empty/whitespace-only pattern.
    EmptyPattern,
}

/// Why a `repo.has.meta` argument set is not admissible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepoMetaArgError {
    /// Not exactly one positional argument.
    WrongArity,
    /// The single argument was not an admitted meta shape — i.e. a numeric arg.
    UnsupportedArg,
    /// The argument carried an empty/whitespace-only key (or bare empty token).
    EmptyKey,
}

/// Parse a metadata token into an exact or regex pattern.
///
/// A token wrapped in `/.../ ` delimiters (a leading AND trailing slash) is a
/// regex; the delimiters are stripped via `strip_prefix`/`strip_suffix` (UTF-8
/// safe, arithmetic-free) and `None` — a missing delimiter — is exactly the
/// exact-string case. Behaviour matches the prior `token[1..len-1]` slice
/// (e.g. `//` -> `Regex("")`).
///
/// Fail-closed trade-off: a slash-terminated value such as `/usr/` is treated as
/// regex syntax even though it could be an exact path. On an exact-string
/// substrate we refuse ambiguous regex-shaped input rather than silently matching
/// the literal slashes. Metadata keys/values and contributor identities are not
/// slash-wrapped in practice.
fn parse_meta_pattern(token: &str) -> MetaPattern {
    let token = token.trim();
    token
        .strip_prefix('/')
        .and_then(|inner| inner.strip_suffix('/'))
        .map_or_else(
            || MetaPattern::Exact(token.to_string()),
            |inner| MetaPattern::Regex(inner.to_string()),
        )
}

/// Parse a `repo.has.meta(key:value)` argument.
///
/// Single owner of the `repo.has.meta` argument contract.
///
/// Admitted shapes:
/// - `key:value`
/// - `key`
/// - `key:`
/// - `/key/`
/// - `/key/:`
/// - `key:/value/`
/// - `/key/:value`
/// - `/key/:/value/`
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
            let value = if value.trim().is_empty() {
                None
            } else {
                Some(parse_meta_pattern(value))
            };
            Ok(RepoMetaArg {
                key: parse_meta_pattern(name),
                value,
            })
        }
        LqPredicateArg::Keyword(value)
        | LqPredicateArg::Phrase(value)
        | LqPredicateArg::RawString(value) => {
            if value.trim().is_empty() {
                return Err(RepoMetaArgError::EmptyKey);
            }
            Ok(RepoMetaArg {
                key: parse_meta_pattern(value),
                value: None,
            })
        }
        LqPredicateArg::Number(_) => Err(RepoMetaArgError::UnsupportedArg),
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

/// Parse a `repo.has.description(pattern)` argument.
///
/// Admitted shape: exactly one keyword/phrase/raw-string scalar, taken
/// verbatim as the regex pattern source (validated for compilation at
/// execution time). A numeric or `key:value` filter arg, or an empty pattern,
/// is rejected.
pub(crate) fn parse_repo_description_arg(
    args: &[LqPredicateArg],
) -> Result<RepoDescriptionArg, RepoDescriptionArgError> {
    let [arg] = args else {
        return Err(RepoDescriptionArgError::WrongArity);
    };
    match arg {
        LqPredicateArg::Keyword(value)
        | LqPredicateArg::Phrase(value)
        | LqPredicateArg::RawString(value) => {
            if value.trim().is_empty() {
                return Err(RepoDescriptionArgError::EmptyPattern);
            }
            Ok(RepoDescriptionArg {
                pattern: value.clone(),
            })
        }
        LqPredicateArg::Number(_) | LqPredicateArg::Filter { .. } => {
            Err(RepoDescriptionArgError::UnsupportedArg)
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
        [
            LqPredicateArg::Keyword(value)
            | LqPredicateArg::Phrase(value)
            | LqPredicateArg::RawString(value),
        ] => {
            if value.trim().is_empty() {
                return Err(FileOwnerArgError::EmptyOwner);
            }
            Ok(FileOwnerArg {
                owner: Some(value.clone()),
            })
        }
        [LqPredicateArg::Number(_) | LqPredicateArg::Filter { .. }] => {
            Err(FileOwnerArgError::UnsupportedArg)
        }
        [_, _, ..] => Err(FileOwnerArgError::WrongArity),
    }
}

/// Parse a `file.has.contributor` argument.
///
/// Admitted shapes:
/// - `file.has.contributor(<keyword|phrase|raw>)` => exact contributor identity
/// - `file.has.contributor(/.../)` => name/email regex identity pattern
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
            Ok(FileContributorArg {
                // Strip surrounding `/.../` without byte-slicing; `None` (a missing
                // delimiter) is the exact-match case, preserving the prior
                // `trimmed[1..len-1]` behaviour UTF-8-safely and arithmetic-free.
                contributor: value
                    .trim()
                    .strip_prefix('/')
                    .and_then(|i| i.strip_suffix('/'))
                    .map_or_else(
                        || ContributorPattern::Exact(value.clone()),
                        |inner| ContributorPattern::Regex(inner.to_string()),
                    ),
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
            LqPredicateArg::Filter { name, value } if name == "name" => {
                if path_scope.is_some() {
                    return Err(ContentPredicateArgError::DuplicatePathScope);
                }
                path_scope = Some(ContentPathScope {
                    pattern: value.clone(),
                    scope: LqFileScope::NameOnly,
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
                key: MetaPattern::Exact("license".to_string()),
                value: Some(MetaPattern::Exact("MIT".to_string())),
            })
        );
        // SGX-03: bare `key` is key-existence (value: None).
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Keyword("license".to_string())]),
            Ok(RepoMetaArg {
                key: MetaPattern::Exact("license".to_string()),
                value: None,
            })
        );
        assert_eq!(parse_repo_meta_arg(&[]), Err(RepoMetaArgError::WrongArity));
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "  ".to_string(),
                value: "MIT".to_string(),
            }]),
            Err(RepoMetaArgError::EmptyKey)
        );
        // SGX-03: a bare empty key still fails closed.
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Keyword("  ".to_string())]),
            Err(RepoMetaArgError::EmptyKey)
        );
        // A numeric arg is still unsupported.
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Number(7)]),
            Err(RepoMetaArgError::UnsupportedArg)
        );
    }

    #[test]
    fn repo_meta_arg_admits_tag_existence_shape() {
        // SGX-03: `repo:has.meta(tag:)` (empty value) is the key-existence shape —
        // the key must be PRESENT with any value, NOT an empty-string match.
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "tag".to_string(),
                value: String::new(),
            }]),
            Ok(RepoMetaArg {
                key: MetaPattern::Exact("tag".to_string()),
                value: None,
            })
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "license".to_string(),
                value: "   ".to_string(),
            }]),
            Ok(RepoMetaArg {
                key: MetaPattern::Exact("license".to_string()),
                value: None,
            })
        );
    }

    #[test]
    fn repo_meta_arg_admits_slash_delimited_regex_shapes() {
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "/key/".to_string(),
                value: "/value/".to_string(),
            }]),
            Ok(RepoMetaArg {
                key: MetaPattern::Regex("key".to_string()),
                value: Some(MetaPattern::Regex("value".to_string())),
            })
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "license".to_string(),
                value: "/apache.*/".to_string(),
            }]),
            Ok(RepoMetaArg {
                key: MetaPattern::Exact("license".to_string()),
                value: Some(MetaPattern::Regex("apache.*".to_string())),
            })
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "/key/".to_string(),
                value: "plain".to_string(),
            }]),
            Ok(RepoMetaArg {
                key: MetaPattern::Regex("key".to_string()),
                value: Some(MetaPattern::Exact("plain".to_string())),
            })
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Keyword("/license/".to_string())]),
            Ok(RepoMetaArg {
                key: MetaPattern::Regex("license".to_string()),
                value: None,
            })
        );
        assert_eq!(
            parse_repo_meta_arg(&[LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "/usr/bin".to_string(),
            }]),
            Ok(RepoMetaArg {
                key: MetaPattern::Exact("path".to_string()),
                value: Some(MetaPattern::Exact("/usr/bin".to_string())),
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
        assert_eq!(
            canonicalize_predicate_call(
                "repo.has.path",
                &[LqPredicateArg::Keyword("src/lib.rs".to_string())],
            ),
            Ok(Some(CanonicalPredicateCall {
                name: "repo.has.file",
                args: vec![LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/lib.rs".to_string(),
                }],
            }))
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
        assert_eq!(
            canonicalize_predicate_call(
                "repo.contains.file",
                &[LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "lib.rs".to_string(),
                }],
            ),
            Ok(Some(CanonicalPredicateCall {
                name: "repo.has.file",
                args: vec![LqPredicateArg::Filter {
                    name: "name".to_string(),
                    value: "lib.rs".to_string(),
                }],
            }))
        );

        // Scalar shorthand forwards as-is; the RepoFileGate executor treats a
        // bare scalar as a path shorthand.
        assert_eq!(
            canonicalize_predicate_call(
                "repo.contains.file",
                &[LqPredicateArg::Keyword("src/lib.rs".to_string())],
            ),
            Ok(Some(CanonicalPredicateCall {
                name: "repo.has.file",
                args: vec![LqPredicateArg::Keyword("src/lib.rs".to_string())],
            }))
        );
    }

    #[test]
    fn canonicalize_repo_contains_path_alias_rewrites_to_path_filter() {
        // `repo.contains.path` is the alias-of-alias of `repo.has.path`; it
        // collapses directly to `repo.has.file(path:...)` so single-level
        // canonicalization resolves a registry kind.
        assert_eq!(
            canonicalize_predicate_call(
                "repo.contains.path",
                &[LqPredicateArg::Keyword("src/lib.rs".to_string())],
            ),
            Ok(Some(CanonicalPredicateCall {
                name: "repo.has.file",
                args: vec![LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/lib.rs".to_string(),
                }],
            }))
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
    fn repo_file_matchers_admit_path_and_content() {
        // SGX-01: `repo:has.file(path:... content:...)` is now a real correlated
        // matcher set — path AND content are both admitted (and AND-ed per-doc by
        // the executor). A `/regex/` content stays a regex via the executor's
        // content_leaf_from_scalar path; the parser just carries the raw value.
        let constraint = match parse_repo_file_matchers(&[
            LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src/lib.rs".to_string(),
            },
            LqPredicateArg::Filter {
                name: "content".to_string(),
                value: "needle".to_string(),
            },
        ]) {
            Ok(constraint) => constraint,
            Err(err) => panic!("path+content must be admitted, got {err:?}"),
        };
        assert!(matches!(
            constraint.matchers.as_slice(),
            [RepoFileMatcher::Path(_), RepoFileMatcher::Content(v)] if v == "needle"
        ));
    }

    #[test]
    fn repo_file_matchers_reject_empty_content_value() {
        // An empty/whitespace content value is not a usable matcher — fail closed.
        assert!(matches!(
            parse_repo_file_matchers(&[
                LqPredicateArg::Filter {
                    name: "path".to_string(),
                    value: "src/lib.rs".to_string(),
                },
                LqPredicateArg::Filter {
                    name: "content".to_string(),
                    value: "  ".to_string(),
                },
            ]),
            Err(RepoFileArgError::EmptyContentValue)
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
                contributor: ContributorPattern::Exact("alice".to_string()),
            })
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Phrase("carol@example.com".to_string())]),
            Ok(FileContributorArg {
                contributor: ContributorPattern::Exact("carol@example.com".to_string()),
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
    fn file_contributor_arg_accepts_slash_delimited_regex_shape() {
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword("/ali.*/".to_string())]),
            Ok(FileContributorArg {
                contributor: ContributorPattern::Regex("ali.*".to_string()),
            })
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Phrase("/carol@.*/".to_string())]),
            Ok(FileContributorArg {
                contributor: ContributorPattern::Regex("carol@.*".to_string()),
            })
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword("a/b".to_string())]),
            Ok(FileContributorArg {
                contributor: ContributorPattern::Exact("a/b".to_string()),
            })
        );
        assert_eq!(
            parse_file_contributor_arg(&[LqPredicateArg::Keyword("/x".to_string())]),
            Ok(FileContributorArg {
                contributor: ContributorPattern::Exact("/x".to_string()),
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
        let Ok(constraint) = parse_content_predicate_constraint(&[
            LqPredicateArg::Filter {
                name: "path".to_string(),
                value: "src".to_string(),
            },
            LqPredicateArg::Filter {
                name: "lang".to_string(),
                value: "rust".to_string(),
            },
            LqPredicateArg::Number(123),
        ]) else {
            assert!(false, "scoped number content predicate accepted");
            return;
        };
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
    fn content_predicate_constraint_accepts_name_scope_filter() {
        let Ok(constraint) = parse_content_predicate_constraint(&[
            LqPredicateArg::Filter {
                name: "name".to_string(),
                value: "colors.md".to_string(),
            },
            LqPredicateArg::Phrase("lemon yellow banana".to_string()),
        ]) else {
            assert!(false, "name-scoped content predicate accepted");
            return;
        };
        assert_eq!(
            constraint.path_scope,
            Some(ContentPathScope {
                pattern: "colors.md".to_string(),
                scope: LqFileScope::NameOnly,
            })
        );
        assert_eq!(
            constraint.content,
            ContentScalarArg::Phrase("lemon yellow banana".to_string())
        );
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
        let Ok(constraint) = parse_repo_file_matchers(&args) else {
            assert!(false, "path/name accepted");
            return;
        };
        assert!(matches!(
            constraint.matchers.as_slice(),
            [RepoFileMatcher::Path(_), RepoFileMatcher::Name(_)]
        ));
    }

    #[test]
    fn repo_file_matchers_accept_single_scalar_path_shorthand() {
        for arg in [
            LqPredicateArg::Keyword("src/lib.rs".to_string()),
            LqPredicateArg::Phrase("src/lib.rs".to_string()),
            LqPredicateArg::RawString("src/lib.rs".to_string()),
        ] {
            let Ok(constraint) = parse_repo_file_matchers(&[arg]) else {
                assert!(false, "scalar path accepted");
                return;
            };
            assert!(matches!(
                constraint.matchers.as_slice(),
                [RepoFileMatcher::Path(v)] if v == "src/lib.rs"
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
        let Ok(constraint) = parse_repo_file_matchers(&args) else {
            assert!(false, "lang accepted");
            return;
        };
        assert!(matches!(
            constraint.matchers.as_slice(),
            [RepoFileMatcher::Language(v)] if v == "rust"
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
