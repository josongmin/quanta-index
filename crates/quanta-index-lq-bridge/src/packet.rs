use quanta_index_contract_base::{
    BridgeCandidatePacket, BridgeScope, BridgeTarget, GenerationPin, LexicalCandidate,
    TextQueryRequest, TextQuerySyntax,
};

use crate::TRANSLATOR_VERSION;

#[must_use]
pub fn export_bridge_candidate_packet(
    target: BridgeTarget,
    scope: BridgeScope,
    generation: &GenerationPin,
    request: &TextQueryRequest,
    candidates: Vec<LexicalCandidate>,
) -> BridgeCandidatePacket {
    let sourcegraph = matches!(request.syntax, TextQuerySyntax::Sourcegraph);
    BridgeCandidatePacket {
        target,
        scope,
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        manifest_generation: generation.manifest_generation,
        source_syntax: sourcegraph.then(|| request.query_text.clone()),
        translator_version: sourcegraph.then(|| TRANSLATOR_VERSION.to_string()),
        candidates,
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract_base::{
        BridgeScope, BridgeTarget, GenerationPin, LexicalCandidate, ManifestGeneration, RepoId,
        RepoRelativePath, RevisionId, TextQueryRequest, TextQuerySyntax,
    };

    use super::export_bridge_candidate_packet;

    fn pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo-bridge"),
            RevisionId::new("rev-bridge"),
            ManifestGeneration::new(17),
        )
    }

    fn candidate() -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: "c1".to_string(),
            repo_id: RepoId::new("repo-bridge"),
            revision_id: RevisionId::new("rev-bridge"),
            manifest_generation: ManifestGeneration::new(17),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            start_line: 1,
            end_line: 1,
            score: 1.0,
            snippet: "needle".to_string(),
        }
    }

    #[test]
    fn sourcegraph_request_exports_metadata() {
        let request = TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "repo:acme/foo needle".to_string(),
            generation: Some(pin()),
            generation_selector: None,
            top_k: 50,
        };
        let packet = export_bridge_candidate_packet(
            BridgeTarget::CodeQl,
            BridgeScope::Lexical,
            &pin(),
            &request,
            vec![candidate()],
        );
        assert_eq!(
            packet.source_syntax.as_deref(),
            Some("repo:acme/foo needle")
        );
        assert_eq!(
            packet.translator_version.as_deref(),
            Some(crate::TRANSLATOR_VERSION)
        );
        assert_eq!(packet.candidates.len(), 1);
    }

    #[test]
    fn lq_request_omits_sourcegraph_metadata() {
        let request = TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            generation: Some(pin()),
            generation_selector: None,
            top_k: 50,
        };
        let packet = export_bridge_candidate_packet(
            BridgeTarget::CodeQl,
            BridgeScope::Semantic,
            &pin(),
            &request,
            vec![candidate()],
        );
        assert_eq!(packet.source_syntax, None);
        assert_eq!(packet.translator_version, None);
        assert_eq!(packet.scope, BridgeScope::Semantic);
    }
}
