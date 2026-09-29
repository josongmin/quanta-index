//! LXE-09 structural domain service.
//!
//! Routes a `StructuralQueryRequest` through an
//! `Arc<dyn StructuralProducerPort>` and maps readiness signals to typed
//! `StructuralError` variants with stable wire codes.

use std::sync::Arc;

use super::inbound::{StructuralQueryRequest, StructuralQueryResponse};
use super::outbound::{StructuralError, StructuralProducerPort, StructuralReadiness};
use super::policy::StructuralPolicy;
use crate::{CoreError, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1};

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
        budget: &RequestBudgetV1,
    ) -> Result<StructuralQueryResponse, StructuralError> {
        structural_checkpoint(budget, "structural:readiness")?;
        if self.policy.default_readiness_check {
            let readiness = self.producer.readiness(request);
            structural_checkpoint(budget, "structural:readiness-return")?;
            match readiness {
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
                StructuralReadiness::InvalidRequest(message) => {
                    return Err(StructuralError::InvalidRequest(message.into()));
                }
                StructuralReadiness::ProducerExecution(message) => {
                    return Err(StructuralError::ProducerExecution(message.into()));
                }
            }
        }
        structural_checkpoint(budget, "structural:producer")?;
        let result = self.producer.execute(request, budget);
        structural_checkpoint(budget, "structural:producer-return")?;
        let candidates = result?;
        Ok(StructuralQueryResponse { candidates })
    }
}

/// Preserve the request's wire interruption code across the domain port.
pub fn structural_checkpoint(
    budget: &RequestBudgetV1,
    stage: &'static str,
) -> Result<(), StructuralError> {
    budget.checkpoint(stage).map_err(|err| match err {
        CoreError::Typed { code, message } if code == REQUEST_CANCELLED_CODE => {
            StructuralError::RequestCancelled(message)
        }
        CoreError::Typed { code, message } if code == REQUEST_DEADLINE_EXCEEDED_CODE => {
            StructuralError::RequestDeadlineExceeded(message)
        }
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            StructuralError::ProducerExecution(format!("unexpected budget error: {other}"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Arc, StructuralError, StructuralProducerPort, StructuralQueryRequest,
        StructuralQueryResponse, StructuralReadiness, StructuralService,
    };
    use crate::RequestBudgetV1;
    use crate::domains::structural::{StructuralMatchBinding, StructuralMatchCandidate};
    use quanta_index_contract::{
        GenerationSelector, LqOptions, LqStructuralBlock, RepoId, RevisionId,
    };

    struct FakeProducer {
        readiness: StructuralReadiness,
        candidates: Vec<StructuralMatchCandidate>,
    }

    impl FakeProducer {
        fn with_readiness(readiness: StructuralReadiness) -> Self {
            Self {
                readiness,
                candidates: Vec::new(),
            }
        }

        fn ready_with(candidates: Vec<StructuralMatchCandidate>) -> Self {
            Self {
                readiness: StructuralReadiness::Ready,
                candidates,
            }
        }
    }

    impl StructuralProducerPort for FakeProducer {
        fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
            self.readiness.clone()
        }

        fn execute(
            &self,
            _request: &StructuralQueryRequest,
            _budget: &RequestBudgetV1,
        ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
            Ok(self.candidates.clone())
        }
    }

    fn dummy_request() -> StructuralQueryRequest {
        StructuralQueryRequest {
            pattern: LqStructuralBlock {
                lang: None,
                nodes: Vec::new(),
                exprs: Vec::new(),
            },
            requested_lang: None,
            filters: Vec::new(),
            candidate_scope: None,
            options: LqOptions::defaults(),
            generation: GenerationSelector::Active {
                repo_id: RepoId::new("repo".to_string())
                    .expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev".to_string())
                    .expect("static fixture ID satisfies canonical policy"),
            },
            aux_epoch: quanta_index_contract::AuxEpochV1::GENESIS,
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
        let result = service.query(&request, &RequestBudgetV1::unbounded());
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
        let result = service.query(&request, &RequestBudgetV1::unbounded());
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
        let result = service.query(&request, &RequestBudgetV1::unbounded());
        assert!(matches!(result, Err(StructuralError::ShardUnavailable)));
        assert_eq!(code_or_debug(&result), "STR_SHARD_UNAVAILABLE");
    }

    #[test]
    fn invalid_request_readiness_maps_to_stable_code() {
        let producer = Arc::new(FakeProducer::with_readiness(
            StructuralReadiness::InvalidRequest(
                "search-plane structural producer requires a pinned generation".into(),
            ),
        ));
        let service = StructuralService::new(producer);
        let request = dummy_request();
        let result = service.query(&request, &RequestBudgetV1::unbounded());
        assert!(matches!(
            result,
            Err(StructuralError::InvalidRequest(ref message))
                if message == "search-plane structural producer requires a pinned generation"
        ));
        assert_eq!(code_or_debug(&result), "STR_INVALID_REQUEST");
    }

    #[test]
    fn producer_execution_readiness_maps_to_stable_code() {
        let producer = Arc::new(FakeProducer::with_readiness(
            StructuralReadiness::ProducerExecution(
                "structural ledger poisoned during readiness: simulated".into(),
            ),
        ));
        let service = StructuralService::new(producer);
        let request = dummy_request();
        let result = service.query(&request, &RequestBudgetV1::unbounded());
        assert!(matches!(
            result,
            Err(StructuralError::ProducerExecution(ref message))
                if message == "structural ledger poisoned during readiness: simulated"
        ));
        assert_eq!(code_or_debug(&result), "STR_PRODUCER_EXECUTION_FAILED");
    }

    #[test]
    fn policy_returns_configured_value_kills_default_replacement_mutation() {
        // `cargo mutants` line 42: replace `self.policy` return with
        // `Default::default()`. Default = (256, true). Using `with_policy`
        // to install a distinguishable policy and asserting equality kills
        // the mutation.
        use super::super::policy::StructuralPolicy;
        let custom = StructuralPolicy {
            max_bindings_per_match: 7,
            default_readiness_check: false,
        };
        let producer = Arc::new(FakeProducer::ready_with(Vec::new()));
        let service = StructuralService::with_policy(producer, custom);
        assert_eq!(service.policy(), custom);
        assert_ne!(service.policy(), StructuralPolicy::default());
    }

    #[test]
    fn ready_returns_candidates() {
        let expected = vec![StructuralMatchCandidate {
            candidate_id: "chunk-1".to_string(),
            pattern_start_byte: 0,
            pattern_end_byte: 4,
            bindings: vec![StructuralMatchBinding {
                metavariable: "$X".to_string(),
                start_byte: 0,
                end_byte: 4,
                start_line: 1,
                end_line: 1,
            }],
        }];
        let producer = Arc::new(FakeProducer::ready_with(expected.clone()));
        let service = StructuralService::new(producer);
        let request = dummy_request();
        let response = service.query(&request, &RequestBudgetV1::unbounded());
        assert!(
            response.is_ok(),
            "expected ready structural response, got {response:?}"
        );
        if let Ok(response) = response {
            assert_eq!(response.candidates, expected);
        }
    }

    #[test]
    fn execute_error_is_forwarded_verbatim() {
        struct ErrorProducer;

        impl StructuralProducerPort for ErrorProducer {
            fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
                StructuralReadiness::Ready
            }

            fn execute(
                &self,
                _request: &StructuralQueryRequest,
                _budget: &RequestBudgetV1,
            ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
                Err(StructuralError::LangNotSupported("java".to_string()))
            }
        }

        let service = StructuralService::new(Arc::new(ErrorProducer));
        let request = dummy_request();
        let result = service.query(&request, &RequestBudgetV1::unbounded());
        assert!(matches!(
            result,
            Err(StructuralError::LangNotSupported(lang)) if lang == "java"
        ));
    }

    #[test]
    fn cancelled_request_is_refused_before_producer_execution() {
        let service = StructuralService::new(Arc::new(FakeProducer::ready_with(Vec::new())));
        let budget = RequestBudgetV1::unbounded();
        budget.cancel_handle().cancel();
        let error = service
            .query(&dummy_request(), &budget)
            .expect_err("cancelled request");
        assert_eq!(error.code(), crate::REQUEST_CANCELLED_CODE);
    }

    #[test]
    fn cancellation_during_producer_execution_is_not_an_empty_success() {
        struct CancellingProducer;
        impl StructuralProducerPort for CancellingProducer {
            fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
                StructuralReadiness::Ready
            }

            fn execute(
                &self,
                _request: &StructuralQueryRequest,
                budget: &RequestBudgetV1,
            ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
                budget.cancel_handle().cancel();
                Ok(Vec::new())
            }
        }

        let service = StructuralService::new(Arc::new(CancellingProducer));
        let error = service
            .query(&dummy_request(), &RequestBudgetV1::unbounded())
            .expect_err("cancellation during producer execution must be returned");
        assert_eq!(error.code(), crate::REQUEST_CANCELLED_CODE);
    }

    #[test]
    fn cancellation_during_readiness_keeps_request_code() {
        struct CancellingReadinessProducer(crate::CancelHandleV1);
        impl StructuralProducerPort for CancellingReadinessProducer {
            fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
                self.0.cancel();
                StructuralReadiness::GenerationNotReady
            }

            fn execute(
                &self,
                _request: &StructuralQueryRequest,
                _budget: &RequestBudgetV1,
            ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
                Err(StructuralError::ProducerExecution(
                    "cancelled request must not execute".to_owned(),
                ))
            }
        }

        let budget = RequestBudgetV1::unbounded();
        let service = StructuralService::new(Arc::new(CancellingReadinessProducer(
            budget.cancel_handle(),
        )));
        let error = service
            .query(&dummy_request(), &budget)
            .expect_err("cancelled readiness");
        assert_eq!(error.code(), crate::REQUEST_CANCELLED_CODE);
    }

    #[test]
    fn expired_request_preserves_deadline_code() {
        let service = StructuralService::new(Arc::new(FakeProducer::ready_with(Vec::new())));
        let budget = RequestBudgetV1::until(std::time::Instant::now());
        let error = service
            .query(&dummy_request(), &budget)
            .expect_err("expired request");
        assert_eq!(error.code(), crate::REQUEST_DEADLINE_EXCEEDED_CODE);
    }
}
