//! Canonical persisted overlay-family identities.
/// One overlay family, by the file it lives in.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum OverlayFamily {
    /// The `FullBundle` repo-metadata payload (fork, archived, visibility,
    /// contexts).
    RepoMetadata,
    /// Latest committer time per source repo.
    CommitRecency,
    /// `key:value` metadata per source repo.
    Meta,
    /// Topics per source repo.
    Topic,
    /// Description per source repo.
    Description,
    /// Owners per file.
    FileOwnership,
    /// Contributors per file.
    Contributor,
}

impl OverlayFamily {
    /// Every family, in manifest order.
    pub(crate) const ALL: [Self; 7] = [
        Self::RepoMetadata,
        Self::CommitRecency,
        Self::Meta,
        Self::Topic,
        Self::Description,
        Self::FileOwnership,
        Self::Contributor,
    ];

    /// The family's file name inside the generation directory.
    pub(crate) const fn file_name(self) -> &'static str {
        match self {
            Self::RepoMetadata => "repo-metadata.cbor",
            Self::CommitRecency => "repo-commit-recency.cbor",
            Self::Meta => "repo-meta.cbor",
            Self::Topic => "repo-topic.cbor",
            Self::Description => "repo-description.cbor",
            Self::FileOwnership => "file-ownership.cbor",
            Self::Contributor => "file-contributor.cbor",
        }
    }

    /// The family a manifest entry names, if any.
    pub(crate) fn from_file_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|family| family.file_name() == name)
    }
}
