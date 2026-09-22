//! What one query read (plan §7.1 `ReadIdentity`): the pin, the sealed
//! artifacts, the auxiliary epochs and the per-domain evidence.
//!
//! V2 (S21-05) adds the candidate commitment, the activation epoch and
//! the evidence map the read view fixes for the request.

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

/// The physical resource group one domain's evidence points at.
///
/// Several declared domains can share one physical handle (every
/// `repo-metadata:*` domain is served by the lexical generation's
/// handle); the evidence names the group so handle count and evidence
/// cardinality can never be confused.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReadResourceGroupV2 {
    LexicalTrack,
    SemanticTrack,
    HistoryEpoch,
    RuntimeEpoch,
    StructuralEpoch,
    RepoMapSnapshot,
}

impl ReadResourceGroupV2 {
    /// The group's name in messages.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::LexicalTrack => "lexical-track",
            Self::SemanticTrack => "semantic-track",
            Self::HistoryEpoch => "history-epoch",
            Self::RuntimeEpoch => "runtime-epoch",
            Self::StructuralEpoch => "structural-epoch",
            Self::RepoMapSnapshot => "repo-map-snapshot",
        }
    }
}

impl fmt::Display for ReadResourceGroupV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// The evidence for one declared domain: exactly one entry per declared
/// domain, naming the physical resource group it executes against and
/// what was pinned there.
///
/// Domains that share a physical handle (the `repo-metadata:*` domains
/// over the lexical handle) carry separate evidence entries naming the
/// same group and artifact: evidence cardinality follows domains, not
/// handles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainReadEvidenceV2 {
    pub domain: ReadDomainV1,
    pub resource_group: ReadResourceGroupV2,
    /// The pinned artifact's identity (manifest digest, or the `RepoMap`
    /// candidate commitment as its wire string).
    pub artifact: Option<String>,
    /// The activation epoch the acquisition observed, when the group has
    /// one (the `RepoMap` snapshot).
    pub activation_epoch: Option<u64>,
    /// The auxiliary epoch pinned, when the domain is an epoch-named
    /// auxiliary snapshot.
    pub aux_epoch: Option<AuxEpochV1>,
}

/// What one query read: the dependency vector the read view fixed.
///
/// `domains` is the declared set; every other field is present exactly
/// when its domain is in the set and describes what was pinned for it.
/// `evidence` holds exactly one entry per declared domain — never fewer,
/// never one for an undeclared domain. Different domains are supplied by
/// different producers at different times and are never claimed to be
/// one instant; what the identity asserts is that every one of them
/// belongs to `pin`'s generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadIdentityV2 {
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
    /// The `RepoMap` candidate commitment the pinned snapshot proved.
    pub repomap_commitment: Option<String>,
    /// The `RepoMap` activation epoch the pinned snapshot observed.
    pub repomap_activation_epoch: Option<u64>,
    /// Exactly one evidence entry per declared domain.
    pub evidence: BTreeMap<ReadDomainV1, DomainReadEvidenceV2>,
}

impl ReadIdentityV2 {
    /// Whether the evidence map is exact for the declared domains: one
    /// entry per declared domain, none for an undeclared one, and every
    /// entry keyed by its own domain.
    ///
    /// By construction the view builds evidence from the declared set;
    /// this is the invariant a route or a test can assert on any
    /// assembled identity.
    #[must_use]
    pub fn evidence_is_exact(&self) -> bool {
        let declared: Vec<ReadDomainV1> = self.domains.iter().collect();
        let evidenced: Vec<ReadDomainV1> = self.evidence.keys().copied().collect();
        declared == evidenced
            && self
                .evidence
                .iter()
                .all(|(domain, evidence)| *domain == evidence.domain)
    }

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
        DomainReadEvidenceV2, LexicalArtifactIdentityV1, ReadIdentityV2, ReadResourceGroupV2,
        SemanticProfileV1, TextNormalizerVersionV1,
    };
    use crate::domains::read_view::domain::{
        ReadDomainV1, RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1, RequiredDomainsV1,
    };

    fn pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(7),
        )
    }

    fn evidence(
        domain: ReadDomainV1,
        resource_group: ReadResourceGroupV2,
        artifact: Option<&str>,
    ) -> DomainReadEvidenceV2 {
        DomainReadEvidenceV2 {
            domain,
            resource_group,
            artifact: artifact.map(str::to_string),
            activation_epoch: None,
            aux_epoch: None,
        }
    }

    #[test]
    fn trace_details_name_the_domains_epochs_and_artifacts_in_order() {
        let lexical = LexicalArtifactIdentityV1 {
            manifest_digest: "lex-digest".to_string(),
            normalizer: TextNormalizerVersionV1 { major: 2, minor: 0 },
            repo_metadata: RepoMetadataAuthoritiesV1::ALL,
        };
        let domains = RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
            .with(ReadDomainV1::SemanticTrack)
            .with(ReadDomainV1::History);
        let identity = ReadIdentityV2 {
            pin: pin(),
            domains,
            lexical_artifact: Some(lexical.manifest_digest.clone()),
            semantic_artifact: Some("sem-digest".to_string()),
            aux_epochs: BTreeMap::from([(ReadDomainV1::History, AuxEpochV1::new(3))]),
            normalizer_version: Some(lexical.normalizer),
            profile: Some(SemanticProfileV1 {
                model_id: "model".to_string(),
                model_revision: None,
            }),
            repomap_commitment: None,
            repomap_activation_epoch: None,
            evidence: domains
                .iter()
                .map(|domain| {
                    let entry = match domain {
                        ReadDomainV1::LexicalTrack => evidence(
                            domain,
                            ReadResourceGroupV2::LexicalTrack,
                            Some("lex-digest"),
                        ),
                        ReadDomainV1::SemanticTrack => evidence(
                            domain,
                            ReadResourceGroupV2::SemanticTrack,
                            Some("sem-digest"),
                        ),
                        ReadDomainV1::History => DomainReadEvidenceV2 {
                            aux_epoch: Some(AuxEpochV1::new(3)),
                            ..evidence(domain, ReadResourceGroupV2::HistoryEpoch, None)
                        },
                        // The declared set is fixed above; the remaining
                        // variants cannot occur in this test's set.
                        ReadDomainV1::RepoMap
                        | ReadDomainV1::RuntimeOverlay
                        | ReadDomainV1::StructuralChunkUniverse
                        | ReadDomainV1::RepoMetadata(_) => {
                            evidence(domain, ReadResourceGroupV2::LexicalTrack, None)
                        }
                    };
                    (domain, entry)
                })
                .collect(),
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
        assert!(identity.evidence_is_exact());
    }

    #[test]
    fn an_identity_without_auxiliary_reads_says_so() {
        let identity = ReadIdentityV2 {
            pin: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(1),
            ),
            domains: RequiredDomainsV1::of(ReadDomainV1::RepoMap),
            lexical_artifact: None,
            semantic_artifact: None,
            aux_epochs: BTreeMap::new(),
            normalizer_version: None,
            profile: None,
            repomap_commitment: Some("commitment".to_string()),
            repomap_activation_epoch: Some(2),
            evidence: BTreeMap::from([(
                ReadDomainV1::RepoMap,
                evidence(
                    ReadDomainV1::RepoMap,
                    ReadResourceGroupV2::RepoMapSnapshot,
                    Some("commitment"),
                ),
            )]),
        };
        assert_eq!(
            identity.trace_details(),
            vec![
                "read_view.domains=repo-map".to_string(),
                "read_view.epochs=-".to_string(),
                "read_view.pin=repo@rev#1".to_string(),
            ]
        );
        assert!(identity.evidence_is_exact());
    }

    #[test]
    fn evidence_is_exact_rejects_a_missing_or_extra_domain() {
        let mut identity = ReadIdentityV2 {
            pin: pin(),
            domains: RequiredDomainsV1::of(ReadDomainV1::LexicalTrack).with(ReadDomainV1::RepoMap),
            lexical_artifact: Some("lex-digest".to_string()),
            semantic_artifact: None,
            aux_epochs: BTreeMap::new(),
            normalizer_version: None,
            profile: None,
            repomap_commitment: Some("commitment".to_string()),
            repomap_activation_epoch: Some(1),
            evidence: BTreeMap::from([
                (
                    ReadDomainV1::LexicalTrack,
                    evidence(
                        ReadDomainV1::LexicalTrack,
                        ReadResourceGroupV2::LexicalTrack,
                        Some("lex-digest"),
                    ),
                ),
                (
                    ReadDomainV1::RepoMap,
                    evidence(
                        ReadDomainV1::RepoMap,
                        ReadResourceGroupV2::RepoMapSnapshot,
                        Some("commitment"),
                    ),
                ),
            ]),
        };
        assert!(identity.evidence_is_exact());
        // A declared domain without evidence breaks exactness.
        let _removed = identity.evidence.remove(&ReadDomainV1::RepoMap);
        assert!(!identity.evidence_is_exact());
        // Evidence for an undeclared domain breaks exactness.
        let _inserted = identity.evidence.insert(
            ReadDomainV1::RepoMap,
            evidence(
                ReadDomainV1::RepoMap,
                ReadResourceGroupV2::RepoMapSnapshot,
                Some("commitment"),
            ),
        );
        identity.domains = RequiredDomainsV1::of(ReadDomainV1::LexicalTrack);
        assert!(!identity.evidence_is_exact());
    }

    #[test]
    fn shared_physical_groups_are_named_by_each_domain() {
        // Every repo-metadata domain shares the lexical handle; evidence
        // cardinality follows domains, not handles.
        let domains = RequiredDomainsV1::of(ReadDomainV1::LexicalTrack)
            .with(ReadDomainV1::RepoMetadata(
                RepoMetadataAuthorityV1::FileOwnership,
            ))
            .with(ReadDomainV1::RepoMetadata(
                RepoMetadataAuthorityV1::Contributor,
            ));
        let identity = ReadIdentityV2 {
            pin: pin(),
            domains,
            lexical_artifact: Some("lex-digest".to_string()),
            semantic_artifact: None,
            aux_epochs: BTreeMap::new(),
            normalizer_version: None,
            profile: None,
            repomap_commitment: None,
            repomap_activation_epoch: None,
            evidence: domains
                .iter()
                .map(|domain| {
                    (
                        domain,
                        evidence(
                            domain,
                            ReadResourceGroupV2::LexicalTrack,
                            Some("lex-digest"),
                        ),
                    )
                })
                .collect(),
        };
        assert!(identity.evidence_is_exact());
        assert_eq!(identity.evidence.len(), 3);
        assert!(
            identity
                .evidence
                .values()
                .all(|evidence| evidence.resource_group == ReadResourceGroupV2::LexicalTrack)
        );
    }
}
