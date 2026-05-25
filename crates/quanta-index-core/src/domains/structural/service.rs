//! LXE-09 structural domain service.
//!
//! Routes a `StructuralQueryRequest` through an
//! `Arc<dyn StructuralProducerPort>` and maps readiness signals to typed
//! `StructuralError` variants with stable wire codes.

use std::sync::Arc;

use super::inbound::{StructuralQueryRequest, StructuralQueryResponse};
use super::outbound::{StructuralError, StructuralProducerPort, StructuralReadiness};
use super::policy::StructuralPolicy;

/// Service that gates parse-tree-backed structural execution behind a
/// readiness check.
///
/// Holds the producer behind a trait object per DIP at the composition seam —
/// concrete adapters are named only by the composition root.
pub struct StructuralService {
    producer: Arc<dyn StructuralProducerPort + Send + Sync>,
    policy: StructuralPolicy,
}

impl StructuralService {
    #[must_use]
    pub fn new(producer: Arc<dyn StructuralProducerPort + Send + Sync>) -> Self {
        Self {
            producer,
            policy: StructuralPolicy::defaults(),
        }
    }

    #[must_use]
    pub fn with_policy(
        producer: Arc<dyn StructuralProducerPort + Send + Sync>,
        policy: StructuralPolicy,
    ) -> Self {
        Self { producer, policy }
    }

    #[must_use]
    pub fn policy(&self) -> StructuralPolicy {
        self.policy
    }

    /// Execute a structural request.
    ///
    /// Returns typed [`StructuralError::ParseTreeProducerUnavailable`] (code
    /// `STR_PRODUCER_PARSE_TREE_UNAVAILABLE`) when the producer reports no
    /// parse-tree ops are wired — never an empty `Ok`.
    pub fn query(
        &self,
        request: &StructuralQueryRequest,
    ) -> Result<StructuralQueryResponse, StructuralError> {
        if self.policy.default_readiness_check {
            match self.producer.readiness(request) {
                StructuralReadiness::Ready => {}
                StructuralReadiness::ParseTreeProducerUnavailable => {
                    return Err(StructuralError::ParseTreeProducerUnavailable);
                }
                StructuralReadiness::GenerationNotReady => {
                    return Err(StructuralError::GenerationNotReady);
                }
                StructuralReadiness::ShardUnavailable => {
                    return Err(StructuralError::ShardUnavailable);
                }
            }
        }
        let bindings = self.producer.execute(request)?;
        Ok(StructuralQueryResponse { bindings })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Arc, StructuralError, StructuralProducerPort, StructuralQueryRequest,
        StructuralQueryResponse, StructuralReadiness, StructuralService,
    };
    use quanta_index_contract::{
        GenerationSelector, LqStructuralBlock, RepoId, RevisionId, StructuralBinding,
    };

    struct FakeProducer {
        readiness: StructuralReadiness,
        bindings: Vec<StructuralBinding>,
    }

    impl FakeProducer {
        fn with_readiness(readiness: StructuralReadiness) -> Self {
            Self {
                readiness,
                bindings: Vec::new(),
            }
        }

        fn ready_with(bindings: Vec<StructuralBinding>) -> Self {
            Self {
                readiness: StructuralReadiness::Ready,
                bindings,
            }
        }
    }

    impl StructuralProducerPort for FakeProducer {
        fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
            self.readiness
        }

        fn execute(
            &self,
            _request: &StructuralQueryRequest,
        ) -> Result<Vec<StructuralBinding>, StructuralError> {
            Ok(self.bindings.clone())
        }
    }

    fn dummy_request() -> StructuralQueryRequest {
        StructuralQueryRequest {
            pattern: LqStructuralBlock {
                lang: None,
                nodes: Vec::new(),
            },
            generation: GenerationSelector::Active {
                repo_id: RepoId::new("repo".to_string()),
                revision_id: RevisionId::new("rev".to_string()),
            },
        }
    }

    fn code_or_debug(result: &Result<StructuralQueryResponse, StructuralError>) -> String {
        match result {
            Ok(_) => "unexpected ok".to_string(),
            Err(err) => err.code().to_string(),
        }
    }

    #[test]
    fn parse_tree_unavailable_maps_to_stable_code() {
        let producer = Arc::new(FakeProducer::with_readiness(
            StructuralReadiness::ParseTreeProducerUnavailable,
        ));
        let service = StructuralService::new(producer);
        let request = dummy_request();
        let result = service.query(&request);
        assert!(matches!(
            result,
            Err(StructuralError::ParseTreeProducerUnavailable)
        ));
        assert_eq!(
            code_or_debug(&result),
            "STR_PRODUCER_PARSE_TREE_UNAVAILABLE"
        );
    }

    #[test]
    fn generation_not_ready_maps_to_stable_code() {
        let producer = Arc::new(FakeProducer::with_readiness(
            StructuralReadiness::GenerationNotReady,
        ));
        let service = StructuralService::new(producer);
        let request = dummy_request();
        let result = service.query(&request);
        assert!(matches!(result, Err(StructuralError::GenerationNotReady)));
        assert_eq!(code_or_debug(&result), "STR_GENERATION_NOT_READY");
    }

    #[test]
    fn shard_unavailable_maps_to_stable_code() {
        let producer = Arc::new(FakeProducer::with_readiness(
            StructuralReadiness::ShardUnavailable,
        ));
        let service = StructuralService::new(producer);
        let request = dummy_request();
        let result = service.query(&request);
        assert!(matches!(result, Err(StructuralError::ShardUnavailable)));
        assert_eq!(code_or_debug(&result), "STR_SHARD_UNAVAILABLE");
    }

    #[test]
    fn ready_returns_bindings() {
        let expected = vec![StructuralBinding {
            metavariable: "$X".to_string(),
            start_byte: 0,
            end_byte: 4,
            start_line: 1,
            end_line: 1,
        }];
        let producer = Arc::new(FakeProducer::ready_with(expected.clone()));
        let service = StructuralService::new(producer);
        let request = dummy_request();
        let response = service.query(&request);
        assert!(
            response.is_ok(),
            "expected ready structural response, got {response:?}"
        );
        if let Ok(response) = response {
            assert_eq!(response.bindings, expected);
        }
    }
}
