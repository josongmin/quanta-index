//! The query read view's declaration and identity (plan §5.6 / §7.1).
//!
//! - `domain` — the closed set of domains a query can read and the
//!   bit-set over it.
//! - `predicate` — the one enumeration of executable predicate names,
//!   each with the domain it reads.
//! - `declare` — `RequiredDomainsV1` as a pure function of the route and
//!   the lowered plan.
//! - `identity` — what one query read, for the response trace.
//! - `errors` — the typed refusals the view owns.
//!
//! The acquisition itself — pinning handles, snapshots and epochs for the
//! declared set — is the search plane's; core fixes what is declared and
//! what an acquired view must be able to say about itself.

mod declare;
mod domain;
mod errors;
mod identity;
mod predicate;

pub use declare::{QueryRouteV1, declare_required_domains_v1};
pub use domain::{
    ReadDomainV1, RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1, RequiredDomainsV1,
};
pub use errors::{
    FILE_CONTRIBUTOR_UNAVAILABLE_CODE, FILE_OWNERSHIP_UNAVAILABLE_CODE,
    READ_VIEW_DOMAIN_UNDECLARED_CODE, READ_VIEW_GENERATION_MIX_CODE,
    REPO_COMMIT_RECENCY_UNAVAILABLE_CODE, REPO_DESCRIPTION_UNAVAILABLE_CODE,
    REPO_META_UNAVAILABLE_CODE, REPO_TOPIC_UNAVAILABLE_CODE, RUNTIME_NOT_READY_CODE,
    ReadViewRefusedError,
};
pub use identity::{
    LexicalArtifactIdentityV1, ReadIdentityV1, SemanticProfileV1, TextNormalizerVersionV1,
};
pub use predicate::{
    LexicalPredicateAliasV1, LexicalPredicateFamilyV1, LexicalPredicateV1, lexical_predicate_v1,
};
