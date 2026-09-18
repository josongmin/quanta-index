//! The domains a query reads (plan §5.6), as a closed typed set.
//!
//! A domain is one authority the search plane pins for a request: a
//! sealed track handle, an epoch-named auxiliary snapshot, the `RepoMap`
//! store, or one of the source-repo metadata authorities materialized
//! beside a lexical generation. A plan declares the set it reads; the
//! read view acquires exactly that set and nothing else.

use core::fmt;

/// One source-repo metadata authority materialized beside a lexical
/// generation, keyed by `source_repo_id` (and path for the file ones).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RepoMetadataAuthorityV1 {
    /// `repo.has.commit.after(...)`: the latest committer time per repo.
    CommitRecency,
    /// `repo.has.meta(key:value)`: key/value pairs per repo.
    Meta,
    /// `repo.has.topic(...)`: the topic set per repo.
    Topic,
    /// `repo.has.description(...)`: the description string per repo.
    Description,
    /// `file.has.owner(...)` and `select:file.owners`: owners per path.
    FileOwnership,
    /// `file.has.contributor(...)`: contributor identities per path.
    Contributor,
}

impl RepoMetadataAuthorityV1 {
    /// Every authority, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::CommitRecency,
        Self::Meta,
        Self::Topic,
        Self::Description,
        Self::FileOwnership,
        Self::Contributor,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::CommitRecency => "commit-recency",
            Self::Meta => "meta",
            Self::Topic => "topic",
            Self::Description => "description",
            Self::FileOwnership => "file-ownership",
            Self::Contributor => "contributor",
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Self::CommitRecency => 0b0000_0001,
            Self::Meta => 0b0000_0010,
            Self::Topic => 0b0000_0100,
            Self::Description => 0b0000_1000,
            Self::FileOwnership => 0b0001_0000,
            Self::Contributor => 0b0010_0000,
        }
    }
}

impl fmt::Display for RepoMetadataAuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// The source-repo metadata authorities an opened lexical generation
/// holds: what its handle decoded beside the index.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct RepoMetadataAuthoritiesV1 {
    bits: u8,
}

impl RepoMetadataAuthoritiesV1 {
    /// No authority materialized.
    pub const NONE: Self = Self { bits: 0 };

    /// Every authority materialized.
    pub const ALL: Self = Self { bits: 0b0011_1111 };

    #[must_use]
    pub const fn with(self, authority: RepoMetadataAuthorityV1) -> Self {
        Self {
            bits: self.bits | authority.bit(),
        }
    }

    #[must_use]
    pub const fn contains(self, authority: RepoMetadataAuthorityV1) -> bool {
        self.bits & authority.bit() != 0
    }
}

/// One domain a query reads.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReadDomainV1 {
    /// The sealed lexical generation: inverted index, text authority,
    /// symbol index.
    LexicalTrack,
    /// The sealed semantic generation: vectors, dense index, cluster
    /// memberships.
    SemanticTrack,
    /// The structural authority: the chunk universe and parse trees, at
    /// one epoch.
    StructuralChunkUniverse,
    /// The runtime-metadata authority: dirty overlay, changed docs, facets,
    /// snapshots, edges, at one epoch.
    RuntimeOverlay,
    /// The history authority: commits, refs, tags, diff hunks and the
    /// epoch's text index, at one epoch.
    History,
    /// The `RepoMap` projection store.
    RepoMap,
    /// One source-repo metadata authority beside the lexical generation.
    RepoMetadata(RepoMetadataAuthorityV1),
}

impl ReadDomainV1 {
    /// Every domain, in declaration order.
    pub const ALL: [Self; 12] = [
        Self::LexicalTrack,
        Self::SemanticTrack,
        Self::StructuralChunkUniverse,
        Self::RuntimeOverlay,
        Self::History,
        Self::RepoMap,
        Self::RepoMetadata(RepoMetadataAuthorityV1::CommitRecency),
        Self::RepoMetadata(RepoMetadataAuthorityV1::Meta),
        Self::RepoMetadata(RepoMetadataAuthorityV1::Topic),
        Self::RepoMetadata(RepoMetadataAuthorityV1::Description),
        Self::RepoMetadata(RepoMetadataAuthorityV1::FileOwnership),
        Self::RepoMetadata(RepoMetadataAuthorityV1::Contributor),
    ];

    /// The domain's name in traces and messages.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::LexicalTrack => "lexical",
            Self::SemanticTrack => "semantic",
            Self::StructuralChunkUniverse => "structural",
            Self::RuntimeOverlay => "runtime",
            Self::History => "history",
            Self::RepoMap => "repo-map",
            Self::RepoMetadata(RepoMetadataAuthorityV1::CommitRecency) => {
                "repo-metadata:commit-recency"
            }
            Self::RepoMetadata(RepoMetadataAuthorityV1::Meta) => "repo-metadata:meta",
            Self::RepoMetadata(RepoMetadataAuthorityV1::Topic) => "repo-metadata:topic",
            Self::RepoMetadata(RepoMetadataAuthorityV1::Description) => "repo-metadata:description",
            Self::RepoMetadata(RepoMetadataAuthorityV1::FileOwnership) => {
                "repo-metadata:file-ownership"
            }
            Self::RepoMetadata(RepoMetadataAuthorityV1::Contributor) => "repo-metadata:contributor",
        }
    }

    const fn bit(self) -> u16 {
        match self {
            Self::LexicalTrack => 0b0000_0000_0000_0001,
            Self::SemanticTrack => 0b0000_0000_0000_0010,
            Self::StructuralChunkUniverse => 0b0000_0000_0000_0100,
            Self::RuntimeOverlay => 0b0000_0000_0000_1000,
            Self::History => 0b0000_0000_0001_0000,
            Self::RepoMap => 0b0000_0000_0010_0000,
            Self::RepoMetadata(RepoMetadataAuthorityV1::CommitRecency) => 0b0000_0000_0100_0000,
            Self::RepoMetadata(RepoMetadataAuthorityV1::Meta) => 0b0000_0000_1000_0000,
            Self::RepoMetadata(RepoMetadataAuthorityV1::Topic) => 0b0000_0001_0000_0000,
            Self::RepoMetadata(RepoMetadataAuthorityV1::Description) => 0b0000_0010_0000_0000,
            Self::RepoMetadata(RepoMetadataAuthorityV1::FileOwnership) => 0b0000_0100_0000_0000,
            Self::RepoMetadata(RepoMetadataAuthorityV1::Contributor) => 0b0000_1000_0000_0000,
        }
    }
}

impl fmt::Display for ReadDomainV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// The set of domains one plan reads: a bit-set over [`ReadDomainV1`].
///
/// Declared as a pure function of the route and the lowered plan (see
/// [`super::declare::declare_required_domains_v1`]) and acquired once per
/// request by the read view; a domain outside the set is never opened.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct RequiredDomainsV1 {
    bits: u16,
}

impl RequiredDomainsV1 {
    /// The empty set.
    pub const NONE: Self = Self { bits: 0 };

    /// The set holding exactly `domain`.
    #[must_use]
    pub const fn of(domain: ReadDomainV1) -> Self {
        Self { bits: domain.bit() }
    }

    /// This set with `domain` added.
    #[must_use]
    pub const fn with(self, domain: ReadDomainV1) -> Self {
        Self {
            bits: self.bits | domain.bit(),
        }
    }

    /// Add `domain`.
    pub fn insert(&mut self, domain: ReadDomainV1) {
        self.bits |= domain.bit();
    }

    /// This set joined with `other`.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    #[must_use]
    pub const fn contains(self, domain: ReadDomainV1) -> bool {
        self.bits & domain.bit() != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// The domains in the set, in [`ReadDomainV1::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = ReadDomainV1> {
        ReadDomainV1::ALL
            .into_iter()
            .filter(move |domain| self.contains(*domain))
    }

    /// The source-repo metadata authorities in the set.
    pub fn repo_metadata(self) -> impl Iterator<Item = RepoMetadataAuthorityV1> {
        RepoMetadataAuthorityV1::ALL
            .into_iter()
            .filter(move |authority| self.contains(ReadDomainV1::RepoMetadata(*authority)))
    }
}

impl FromIterator<ReadDomainV1> for RequiredDomainsV1 {
    fn from_iter<I: IntoIterator<Item = ReadDomainV1>>(domains: I) -> Self {
        domains.into_iter().fold(Self::NONE, Self::with)
    }
}

impl fmt::Display for RequiredDomainsV1 {
    /// The domains in [`ReadDomainV1::ALL`] order, comma-separated; `-`
    /// for the empty set.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return formatter.write_str("-");
        }
        let mut first = true;
        for domain in self.iter() {
            if !first {
                formatter.write_str(",")?;
            }
            first = false;
            formatter.write_str(domain.as_code_str())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ReadDomainV1, RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1, RequiredDomainsV1,
    };

    #[test]
    fn every_domain_has_a_distinct_bit_and_name() {
        let mut seen_bits = 0_u16;
        let mut names = std::collections::BTreeSet::new();
        for domain in ReadDomainV1::ALL {
            let bit = domain.bit();
            assert_eq!(bit.count_ones(), 1, "{domain}: one bit");
            assert_eq!(seen_bits & bit, 0, "{domain}: bit already taken");
            seen_bits |= bit;
            assert!(names.insert(domain.as_code_str()), "{domain}: name taken");
        }
        let mut seen_bits = 0_u8;
        for authority in RepoMetadataAuthorityV1::ALL {
            let bit = authority.bit();
            assert_eq!(bit.count_ones(), 1, "{authority}: one bit");
            assert_eq!(seen_bits & bit, 0, "{authority}: bit already taken");
            seen_bits |= bit;
        }
        assert_eq!(RepoMetadataAuthoritiesV1::ALL.bits, seen_bits);
    }

    #[test]
    fn a_set_reads_back_what_was_inserted_in_declaration_order() {
        let set = RequiredDomainsV1::of(ReadDomainV1::History)
            .with(ReadDomainV1::LexicalTrack)
            .with(ReadDomainV1::RepoMetadata(
                RepoMetadataAuthorityV1::FileOwnership,
            ));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![
                ReadDomainV1::LexicalTrack,
                ReadDomainV1::History,
                ReadDomainV1::RepoMetadata(RepoMetadataAuthorityV1::FileOwnership),
            ]
        );
        assert_eq!(
            set.repo_metadata().collect::<Vec<_>>(),
            vec![RepoMetadataAuthorityV1::FileOwnership]
        );
        assert_eq!(
            set.to_string(),
            "lexical,history,repo-metadata:file-ownership"
        );
        assert_eq!(RequiredDomainsV1::NONE.to_string(), "-");
        assert!(!set.contains(ReadDomainV1::SemanticTrack));
        assert_eq!(
            set.union(RequiredDomainsV1::of(ReadDomainV1::SemanticTrack)),
            set.with(ReadDomainV1::SemanticTrack)
        );
        let collected: RequiredDomainsV1 = set.iter().collect();
        assert_eq!(collected, set);
    }

    #[test]
    fn repo_metadata_authorities_answer_membership() {
        let held = RepoMetadataAuthoritiesV1::NONE.with(RepoMetadataAuthorityV1::Meta);
        assert!(held.contains(RepoMetadataAuthorityV1::Meta));
        assert!(!held.contains(RepoMetadataAuthorityV1::Topic));
        for authority in RepoMetadataAuthorityV1::ALL {
            assert!(RepoMetadataAuthoritiesV1::ALL.contains(authority));
            assert!(!RepoMetadataAuthoritiesV1::NONE.contains(authority));
        }
    }
}
