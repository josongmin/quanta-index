//! The executable predicate names and the domain each one reads.
//!
//! This is the one place predicate names are enumerated. The lexical
//! adapter's predicate registry keys its lowering rows by these variants
//! (its tests prove every content/repo variant has exactly one row), the
//! symbol planner names [`LexicalPredicateV1::SymbolHasName`] through the
//! same enum, and the read-view declaration resolves a lowered
//! `LqLeaf::Predicate { name, .. }` here to learn which authority the
//! leaf reads. An alias resolves to its canonical predicate; a name
//! outside both tables is unregistered and reads nothing, because it
//! cannot execute — the adapter refuses it typed.

use super::domain::{ReadDomainV1, RepoMetadataAuthorityV1};

/// Every predicate name the plane executes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LexicalPredicateV1 {
    /// `file.contains(<content>)`: a lexical content leaf.
    FileContains,
    /// `file.has.content(<content>)`: a lexical content leaf.
    FileHasContent,
    /// `repo.has.file(<path> | path:/name:/lang: matchers)`: a repo gate
    /// over the index.
    RepoHasFile,
    /// `repo.has.content(<content>)`: a repo gate over the index.
    RepoHasContent,
    /// `repo.has.commit.after(<timeref>)`: a repo gate over the
    /// commit-recency authority.
    RepoHasCommitAfter,
    /// `repo.has.meta(key:value)`: a repo gate over the repo-meta
    /// authority.
    RepoHasMeta,
    /// `repo.has.topic(<topic>)`: a repo gate over the repo-topic
    /// authority.
    RepoHasTopic,
    /// `repo.has.description(<pattern>)`: a repo gate over the
    /// repo-description authority.
    RepoHasDescription,
    /// `file.has.owner([owner])`: a file gate over the ownership
    /// authority.
    FileHasOwner,
    /// `file.has.contributor(<identity>)`: a file gate over the
    /// contributor authority.
    FileHasContributor,
    /// `symbol.has.name(<name>)`: a symbol-index leaf, planned by the
    /// symbol planner rather than the content/repo registry.
    SymbolHasName,
}

/// Which lowering surface a predicate belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LexicalPredicateFamilyV1 {
    /// Lowered through the content/repo predicate registry.
    ContentOrRepo,
    /// Lowered through the symbol planner.
    Symbol,
}

impl LexicalPredicateV1 {
    /// Every predicate, in declaration order.
    pub const ALL: [Self; 11] = [
        Self::FileContains,
        Self::FileHasContent,
        Self::RepoHasFile,
        Self::RepoHasContent,
        Self::RepoHasCommitAfter,
        Self::RepoHasMeta,
        Self::RepoHasTopic,
        Self::RepoHasDescription,
        Self::FileHasOwner,
        Self::FileHasContributor,
        Self::SymbolHasName,
    ];

    /// The canonical dot-joined name a lowered leaf carries.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FileContains => "file.contains",
            Self::FileHasContent => "file.has.content",
            Self::RepoHasFile => "repo.has.file",
            Self::RepoHasContent => "repo.has.content",
            Self::RepoHasCommitAfter => "repo.has.commit.after",
            Self::RepoHasMeta => "repo.has.meta",
            Self::RepoHasTopic => "repo.has.topic",
            Self::RepoHasDescription => "repo.has.description",
            Self::FileHasOwner => "file.has.owner",
            Self::FileHasContributor => "file.has.contributor",
            Self::SymbolHasName => "symbol.has.name",
        }
    }

    /// The domain the predicate reads when it executes.
    #[must_use]
    pub const fn read_domain(self) -> ReadDomainV1 {
        match self {
            Self::FileContains
            | Self::FileHasContent
            | Self::RepoHasFile
            | Self::RepoHasContent
            | Self::SymbolHasName => ReadDomainV1::LexicalTrack,
            Self::RepoHasCommitAfter => {
                ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::CommitRecency)
            }
            Self::RepoHasMeta => ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::Meta),
            Self::RepoHasTopic => ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::Topic),
            Self::RepoHasDescription => {
                ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::Description)
            }
            Self::FileHasOwner => {
                ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::FileOwnership)
            }
            Self::FileHasContributor => {
                ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::Contributor)
            }
        }
    }

    /// Which lowering surface the predicate belongs to.
    #[must_use]
    pub const fn family(self) -> LexicalPredicateFamilyV1 {
        match self {
            Self::FileContains
            | Self::FileHasContent
            | Self::RepoHasFile
            | Self::RepoHasContent
            | Self::RepoHasCommitAfter
            | Self::RepoHasMeta
            | Self::RepoHasTopic
            | Self::RepoHasDescription
            | Self::FileHasOwner
            | Self::FileHasContributor => LexicalPredicateFamilyV1::ContentOrRepo,
            Self::SymbolHasName => LexicalPredicateFamilyV1::Symbol,
        }
    }

    /// The predicate whose canonical name is `name`, if any.
    #[must_use]
    pub fn from_canonical_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|predicate| predicate.name() == name)
    }
}

/// A native alias that rewrites onto one canonical predicate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LexicalPredicateAliasV1 {
    /// `repo.has.path(<path>)` → `repo.has.file(path:<path>)`.
    RepoHasPath,
    /// `repo.contains.file(...)` → `repo.has.file(...)`.
    RepoContainsFile,
    /// `repo.contains.path(<path>)` → `repo.has.file(path:<path>)`.
    RepoContainsPath,
    /// `file.contains.content(...)` → `file.contains(...)`.
    FileContainsContent,
    /// `repo.contains.content(...)` → `repo.has.content(...)`.
    RepoContainsContent,
    /// `repo.contains.commit.after(...)` → `repo.has.commit.after(...)`.
    RepoContainsCommitAfter,
}

impl LexicalPredicateAliasV1 {
    /// Every alias, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::RepoHasPath,
        Self::RepoContainsFile,
        Self::RepoContainsPath,
        Self::FileContainsContent,
        Self::RepoContainsContent,
        Self::RepoContainsCommitAfter,
    ];

    /// The dot-joined name a lowered leaf carries.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RepoHasPath => "repo.has.path",
            Self::RepoContainsFile => "repo.contains.file",
            Self::RepoContainsPath => "repo.contains.path",
            Self::FileContainsContent => "file.contains.content",
            Self::RepoContainsContent => "repo.contains.content",
            Self::RepoContainsCommitAfter => "repo.contains.commit.after",
        }
    }

    /// The predicate the alias rewrites onto.
    #[must_use]
    pub const fn canonical(self) -> LexicalPredicateV1 {
        match self {
            Self::RepoHasPath | Self::RepoContainsFile | Self::RepoContainsPath => {
                LexicalPredicateV1::RepoHasFile
            }
            Self::FileContainsContent => LexicalPredicateV1::FileContains,
            Self::RepoContainsContent => LexicalPredicateV1::RepoHasContent,
            Self::RepoContainsCommitAfter => LexicalPredicateV1::RepoHasCommitAfter,
        }
    }

    /// The alias whose name is `name`, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|alias| alias.name() == name)
    }
}

/// The predicate a lowered leaf names, by canonical name or alias; `None`
/// for a name outside the executable set.
#[must_use]
pub fn lexical_predicate_v1(name: &str) -> Option<LexicalPredicateV1> {
    LexicalPredicateV1::from_canonical_name(name).or_else(|| {
        LexicalPredicateAliasV1::from_name(name).map(LexicalPredicateAliasV1::canonical)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{LexicalPredicateAliasV1, LexicalPredicateV1, lexical_predicate_v1};

    #[test]
    fn names_are_distinct_across_predicates_and_aliases() {
        let mut names = BTreeSet::new();
        for predicate in LexicalPredicateV1::ALL {
            assert!(names.insert(predicate.name()), "{}: name taken", predicate.name());
        }
        for alias in LexicalPredicateAliasV1::ALL {
            assert!(names.insert(alias.name()), "{}: name taken", alias.name());
        }
    }

    #[test]
    fn every_name_resolves_and_an_alias_resolves_to_its_canonical() {
        for predicate in LexicalPredicateV1::ALL {
            assert_eq!(lexical_predicate_v1(predicate.name()), Some(predicate));
        }
        for alias in LexicalPredicateAliasV1::ALL {
            assert_eq!(lexical_predicate_v1(alias.name()), Some(alias.canonical()));
        }
        assert_eq!(lexical_predicate_v1("repo.has.tag"), None);
        assert_eq!(lexical_predicate_v1(""), None);
    }
}
