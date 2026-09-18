//! What one query read (plan §7.1 `ReadIdentity`): the pin, the sealed
//! artifacts, the auxiliary epochs and the capability versions the read
//! view fixed for the request.

use core::fmt;
use std::collections::BTreeMap;

use quanta_index_contract::{AuxEpochV1, GenerationPin};

use super::domain::{ReadDomainV1, RepoMetadataAuthoritiesV1, RequiredDomainsV1};

/// The text normalizer a lexical generation was built under (QI-BB-011),
/// as its handle reports it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextNormalizerVersionV1 {
    pub major: u16,
    pub minor: u16,
}

impl fmt::Display for TextNormalizerVersionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

/// What an opened lexical generation is: the sealed manifest digest its
/// open proved, the normalizer it was built under, and the source-repo
/// metadata authorities its handle decoded beside the index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalArtifactIdentityV1 {
    pub manifest_digest: String,
    pub normalizer: TextNormalizerVersionV1,
    pub repo_metadata: RepoMetadataAuthoritiesV1,
}

/// The embedding profile a semantic generation's vectors were built with,
/// as its handle reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticProfileV1 {
    pub model_id: String,
    pub model_revision: Option<String>,
}

/// What one query read: the dependency vector the read view fixed.
///
/// `domains` is the declared set; every other field is present exactly
/// when its domain is in the set and describes what was pinned for it.
/// Different domains are supplied by different producers at different
/// times and are never claimed to be one instant; what the identity
/// asserts is that every one of them belongs to `pin`'s generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadIdentityV1 {
    pub pin: GenerationPin,
    pub domains: RequiredDomainsV1,
    /// The lexical track's sealed manifest digest.
    pub lexical_artifact: Option<String>,
    /// The semantic track's sealed manifest digest.
    pub semantic_artifact: Option<String>,
    /// The epoch each auxiliary domain was read at.
    pub aux_epochs: BTreeMap<ReadDomainV1, AuxEpochV1>,
    /// The lexical generation's text normalizer.
    pub normalizer_version: Option<TextNormalizerVersionV1>,
    /// The semantic generation's embedding profile.
    pub profile: Option<SemanticProfileV1>,
}

impl ReadIdentityV1 {
    /// The identity as planner-trace details, one per line, in a fixed
    /// order: `read_view.domains=…`, `read_view.epochs=…`, then the pin
    /// and the artifacts that were pinned.
    #[must_use]
    pub fn trace_details(&self) -> Vec<String> {
        let mut details = vec![
            format!("read_view.domains={}", self.domains),
            format!("read_view.epochs={}", self.epochs_detail()),
            format!(
                "read_view.pin={}@{}#{}",
                self.pin.repo_id.as_str(),
                self.pin.revision_id.as_str(),
                self.pin.manifest_generation.get()
            ),
        ];
        if let Some(digest) = &self.lexical_artifact {
            details.push(format!("read_view.lexical_artifact={digest}"));
        }
        if let Some(normalizer) = &self.normalizer_version {
            details.push(format!("read_view.normalizer={normalizer}"));
        }
        if let Some(digest) = &self.semantic_artifact {
            details.push(format!("read_view.semantic_artifact={digest}"));
        }
        if let Some(profile) = &self.profile {
            details.push(format!(
                "read_view.profile={}@{}",
                profile.model_id,
                profile.model_revision.as_deref().unwrap_or("-")
            ));
        }
        details
    }

    /// `domain:epoch` pairs in domain order, `-` when no auxiliary domain
    /// was read.
    fn epochs_detail(&self) -> String {
        if self.aux_epochs.is_empty() {
            return "-".to_string();
        }
        self.aux_epochs
            .iter()
            .map(|(domain, epoch)| format!("{domain}:{epoch}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use quanta_index_contract::{
        AuxEpochV1, GenerationPin, ManifestGeneration, RepoId, RevisionId,
    };

    use super::{
        LexicalArtifactIdentityV1, ReadIdentityV1, SemanticProfileV1, TextNormalizerVersionV1,
    };
    use crate::domains::read_view::domain::{
        ReadDomainV1, RepoMetadataAuthoritiesV1, RequiredDomainsV1,
    };

    #[test]
    fn trace_details_name_the_domains_epochs_and_artifacts_in_order() {
        let lexical = LexicalArtifactIdentityV1 {
            manifest_digest: "lex-digest".to_string(),
            normalizer: TextNormalizerVersionV1 { major: 2, minor: 0 },
            repo_metadata: RepoMetadataAuthoritiesV1::ALL,
        };
        let identity = ReadIdentityV1 {
            pin: GenerationPin::new(
                RepoId::new("repo"),
                RevisionId::new("rev"),
                ManifestGeneration::new(7),
            ),
            domains: RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
                .with(ReadDomainV1::SemanticTrack)
                .with(ReadDomainV1::History),
            lexical_artifact: Some(lexical.manifest_digest.clone()),
            semantic_artifact: Some("sem-digest".to_string()),
            aux_epochs: BTreeMap::from([(ReadDomainV1::History, AuxEpochV1::new(3))]),
            normalizer_version: Some(lexical.normalizer),
            profile: Some(SemanticProfileV1 {
                model_id: "model".to_string(),
                model_revision: None,
            }),
        };
        assert_eq!(
            identity.trace_details(),
            vec![
                "read_view.domains=lexical,semantic,history".to_string(),
                "read_view.epochs=history:3".to_string(),
                "read_view.pin=repo@rev#7".to_string(),
                "read_view.lexical_artifact=lex-digest".to_string(),
                "read_view.normalizer=2.0".to_string(),
                "read_view.semantic_artifact=sem-digest".to_string(),
                "read_view.profile=model@-".to_string(),
            ]
        );
    }

    #[test]
    fn an_identity_without_auxiliary_reads_says_so() {
        let identity = ReadIdentityV1 {
            pin: GenerationPin::new(
                RepoId::new("repo"),
                RevisionId::new("rev"),
                ManifestGeneration::new(1),
            ),
            domains: RequiredDomainsV1::of(ReadDomainV1::RepoMap),
            lexical_artifact: None,
            semantic_artifact: None,
            aux_epochs: BTreeMap::new(),
            normalizer_version: None,
            profile: None,
        };
        assert_eq!(
            identity.trace_details(),
            vec![
                "read_view.domains=repo-map".to_string(),
                "read_view.epochs=-".to_string(),
                "read_view.pin=repo@rev#1".to_string(),
            ]
        );
    }
}
