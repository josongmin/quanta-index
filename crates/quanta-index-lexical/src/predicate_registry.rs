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
];

pub(crate) const PREDICATE_ALIASES: &[PredicateAliasSpec] = &[
    PredicateAliasSpec {
        alias: "repo.has.path",
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
    fn registry_resolves_repo_has_content_kind() {
        assert_eq!(
            kind_of("repo.has.content"),
            Some(PredicateKind::RepoContentGate)
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
