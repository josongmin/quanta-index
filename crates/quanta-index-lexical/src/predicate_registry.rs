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

use quanta_index_contract::LqPredicateArg;
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
    /// stripping owned there. Note: the planner's arity/shape gate
    /// (`single_string_arg`) and the lowering gate diverge by design on a
    /// `Number` argument — the planner rejects it, lowering coerces it to a
    /// keyword — so the content-arg contract is intentionally *not* a single
    /// shared validator. Only the name→kind dispatch and the repo-file matcher
    /// contract are registry-owned.
    ContentLeaf,
    /// `repo.has.file` — one or more `path:` / `name:` / `lang:` matcher
    /// filters, lowered to a repo-existence gate. Matcher parsing is
    /// registry-owned via [`parse_repo_file_matchers`].
    RepoFileGate,
}

/// One predicate capability row.
pub(crate) struct PredicateSpec {
    /// Canonical dot-joined predicate name (e.g. `repo.has.file`).
    pub name: &'static str,
    /// Lowering target for this predicate.
    pub kind: PredicateKind,
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
];

/// Resolve a predicate name to its lowering [`PredicateKind`], or `None` when
/// the name is outside the executable lexical subset.
pub(crate) fn kind_of(name: &str) -> Option<PredicateKind> {
    PREDICATE_REGISTRY
        .iter()
        .find(|spec| spec.name == name)
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
    /// An argument other than a `path:` / `name:` / `lang:` filter appeared.
    UnsupportedArg,
    /// No `path:` / `name:` / `lang:` matcher was supplied.
    NoMatcher,
    /// A `lang:` matcher value was empty or whitespace-only.
    EmptyLanguageValue,
}

/// Parse `repo.has.file(...)` arguments into validated matchers.
///
/// This is the single owner of the `repo.has.file` argument contract: exactly
/// the `path:`, `name:`, and `lang:` filter shapes are admitted, and at least
/// one matcher is required. Both lexical lowering and planner validation route
/// through here so the contract cannot diverge between them.
pub(crate) fn parse_repo_file_matchers(
    args: &[LqPredicateArg],
) -> Result<RepoFileConstraint, RepoFileArgError> {
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
        let args = vec![LqPredicateArg::Keyword("src".to_string())];
        assert!(matches!(
            parse_repo_file_matchers(&args),
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
