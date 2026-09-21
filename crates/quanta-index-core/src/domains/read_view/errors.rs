//! The typed refusals of a read view: one code per domain that could not
//! be pinned at the request's generation, and one for a mix.
//!
//! The lexical track keeps its `NOT_READY`, the semantic track its
//! `SEMANTIC_GENERATION_NOT_*` codes, the structural authority
//! `STR_GENERATION_NOT_READY` and the history authority its
//! `HISTORY_GENERATION_NOT_READY` / `HISTORY_PRODUCER_UNAVAILABLE`; those
//! are raised by the readiness authorities the view consults. The codes
//! here are the ones the view itself owns.

use core::fmt;

use quanta_index_contract::{GenerationPin, SearchPlaneErrorCodeV2};

use super::domain::{ReadDomainV1, RepoMetadataAuthorityV1};
use crate::domains::auxiliary::AuxiliaryGenerationKeyV1;
use crate::error::CoreError;

/// Wire code for a runtime-metadata read on a generation whose runtime
/// authority is not materialized.
pub const RUNTIME_NOT_READY_CODE: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::RuntimeNotReady;

/// Wire code for a view whose auxiliary snapshot belongs to another
/// generation than the pin: the view refuses to serve a mix rather than
/// claim the parts are one dependency vector.
pub const READ_VIEW_GENERATION_MIX_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::ReadViewGenerationMix;

/// Wire code for a route that reached for a domain its plan did not
/// declare: a search-plane defect, refused rather than acquired late.
pub const READ_VIEW_DOMAIN_UNDECLARED_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::ReadViewDomainUndeclared;

/// Wire code for `repo.has.commit.after(...)` on a generation without a
/// commit-recency authority.
pub const REPO_COMMIT_RECENCY_UNAVAILABLE_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::HistoryRepoCommitRecencyUnavailable;

/// Wire code for `repo.has.meta(...)` on a generation without a repo-meta
/// authority.
pub const REPO_META_UNAVAILABLE_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::RepoMetaUnavailable;

/// Wire code for `repo.has.topic(...)` on a generation without a
/// repo-topic authority.
pub const REPO_TOPIC_UNAVAILABLE_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::RepoTopicUnavailable;

/// Wire code for `repo.has.description(...)` on a generation without a
/// repo-description authority.
pub const REPO_DESCRIPTION_UNAVAILABLE_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::RepoDescriptionUnavailable;

/// Wire code for `file.has.owner(...)` / `select:file.owners` on a
/// generation without a file-ownership authority.
pub const FILE_OWNERSHIP_UNAVAILABLE_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::FileOwnershipUnavailable;

/// Wire code for `file.has.contributor(...)` on a generation without a
/// file-contributor authority.
pub const FILE_CONTRIBUTOR_UNAVAILABLE_CODE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::FileContributorUnavailable;

impl RepoMetadataAuthorityV1 {
    /// The wire code raised when the authority is required but the
    /// generation did not materialize it.
    #[must_use]
    pub const fn unavailable_code(self) -> SearchPlaneErrorCodeV2 {
        match self {
            Self::CommitRecency => REPO_COMMIT_RECENCY_UNAVAILABLE_CODE,
            Self::Meta => REPO_META_UNAVAILABLE_CODE,
            Self::Topic => REPO_TOPIC_UNAVAILABLE_CODE,
            Self::Description => REPO_DESCRIPTION_UNAVAILABLE_CODE,
            Self::FileOwnership => FILE_OWNERSHIP_UNAVAILABLE_CODE,
            Self::Contributor => FILE_CONTRIBUTOR_UNAVAILABLE_CODE,
        }
    }

    /// The authority's name in messages, as the lexical adapter names it
    /// when a predicate reaches an absent shard.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::CommitRecency => "commit recency",
            Self::Meta => "repo metadata",
            Self::Topic => "repo topic",
            Self::Description => "repo description",
            Self::FileOwnership => "file ownership",
            Self::Contributor => "file contributor",
        }
    }

    /// The predicate surface the authority serves, for messages.
    const fn surface(self) -> &'static str {
        match self {
            Self::CommitRecency => "repo.has.commit.after",
            Self::Meta => "repo.has.meta",
            Self::Topic => "repo.has.topic",
            Self::Description => "repo.has.description",
            Self::FileOwnership => "file.has.owner / select:file.owners",
            Self::Contributor => "file.has.contributor",
        }
    }
}

/// Why a read view could not be assembled for a plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadViewRefusedError {
    /// The plan reads a source-repo metadata authority the pinned lexical
    /// generation did not materialize.
    RepoMetadataUnavailable {
        authority: RepoMetadataAuthorityV1,
        pin: GenerationPin,
    },
    /// The plan reads the runtime authority and the pinned generation has
    /// none.
    RuntimeNotReady { pin: GenerationPin },
    /// An auxiliary snapshot offered for `domain` belongs to another
    /// generation than the pin.
    GenerationMix {
        domain: ReadDomainV1,
        pin: GenerationPin,
        offered: AuxiliaryGenerationKeyV1,
    },
    /// A route reached for a domain its plan did not declare.
    DomainUndeclared {
        domain: ReadDomainV1,
        pin: GenerationPin,
    },
}

impl ReadViewRefusedError {
    /// The wire code of the refusal.
    #[must_use]
    pub const fn code(&self) -> SearchPlaneErrorCodeV2 {
        match self {
            Self::RepoMetadataUnavailable { authority, .. } => authority.unavailable_code(),
            Self::RuntimeNotReady { .. } => RUNTIME_NOT_READY_CODE,
            Self::GenerationMix { .. } => READ_VIEW_GENERATION_MIX_CODE,
            Self::DomainUndeclared { .. } => READ_VIEW_DOMAIN_UNDECLARED_CODE,
        }
    }
}

impl fmt::Display for ReadViewRefusedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RepoMetadataUnavailable { authority, pin } => write!(
                formatter,
                "read view: {} requires materialized source-repo {} authority for generation {} of repo={} revision={}",
                authority.surface(),
                authority.describe(),
                pin.manifest_generation.get(),
                pin.repo_id.as_str(),
                pin.revision_id.as_str()
            ),
            Self::RuntimeNotReady { pin } => write!(
                formatter,
                "read view: runtime authority for generation {} of repo={} revision={} is not materialized",
                pin.manifest_generation.get(),
                pin.repo_id.as_str(),
                pin.revision_id.as_str()
            ),
            Self::GenerationMix {
                domain,
                pin,
                offered,
            } => write!(
                formatter,
                "read view: the {domain} snapshot offered for generation {} of repo={} revision={} belongs to generation {} of repo={} revision={}; a view never mixes generations",
                pin.manifest_generation.get(),
                pin.repo_id.as_str(),
                pin.revision_id.as_str(),
                offered.generation.get(),
                offered.repo_id.as_str(),
                offered.revision_id.as_str()
            ),
            Self::DomainUndeclared { domain, pin } => write!(
                formatter,
                "read view: the {domain} domain was not declared for generation {} of repo={} revision={} and is not held by this view",
                pin.manifest_generation.get(),
                pin.repo_id.as_str(),
                pin.revision_id.as_str()
            ),
        }
    }
}

impl std::error::Error for ReadViewRefusedError {}

impl From<ReadViewRefusedError> for CoreError {
    fn from(refused: ReadViewRefusedError) -> Self {
        Self::Typed {
            code: refused.code(),
            message: refused.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{GenerationPin, ManifestGeneration, RepoId, RevisionId};

    use super::{READ_VIEW_GENERATION_MIX_CODE, RUNTIME_NOT_READY_CODE, ReadViewRefusedError};
    use crate::domains::auxiliary::AuxiliaryGenerationKeyV1;
    use crate::domains::read_view::domain::{ReadDomainV1, RepoMetadataAuthorityV1};
    use crate::error::CoreError;

    fn pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(4),
        )
    }

    #[test]
    fn every_refusal_carries_its_domain_code() {
        for authority in RepoMetadataAuthorityV1::ALL {
            let refused = ReadViewRefusedError::RepoMetadataUnavailable {
                authority,
                pin: pin(),
            };
            assert_eq!(refused.code(), authority.unavailable_code());
            let CoreError::Typed { code, message } = CoreError::from(refused) else {
                panic!("a refusal is a typed error");
            };
            assert_eq!(code, authority.unavailable_code());
            assert!(
                message.contains(&format!(
                    "requires materialized source-repo {} authority",
                    authority.describe()
                )),
                "{message}"
            );
        }
        assert_eq!(
            ReadViewRefusedError::RuntimeNotReady { pin: pin() }.code(),
            RUNTIME_NOT_READY_CODE
        );
        let mix = ReadViewRefusedError::GenerationMix {
            domain: ReadDomainV1::History,
            pin: pin(),
            offered: AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev")
                    .expect("static fixture ID satisfies canonical policy"),
                generation: ManifestGeneration::new(3),
            },
        };
        assert_eq!(mix.code(), READ_VIEW_GENERATION_MIX_CODE);
        assert!(mix.to_string().contains("belongs to generation 3"));
    }
}
