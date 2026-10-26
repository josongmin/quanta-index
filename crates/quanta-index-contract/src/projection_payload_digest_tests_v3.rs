use super::*;
use sha2::{Digest as _, Sha256};

fn transition() -> ProjectionTransition {
    ProjectionTransition {
        projection: ProjectionId([1; 16]),
        producer: ProducerId([2; 16]),
        expected: None,
        target_source: SourceCut {
            owners: MapRoot(Some(ObjectRef(Digest([3; 32])))),
            paths: MapRoot(None),
            frontiers: MapRoot(Some(ObjectRef(Digest([4; 32])))),
        },
        schema: SchemaId([5; 16]),
        producer_policy: Digest([6; 32]),
        dependency_context: Digest([7; 32]),
        completeness: ProducerCompleteness::CompleteAffectedDomain {
            certificate: ObjectRef(Digest([8; 32])),
        },
        owners: OwnerMutationSet {
            root: MapRoot(Some(ObjectRef(Digest([9; 32])))),
            count: 17,
            digest: Digest([10; 32]),
        },
        events: Vec::new(),
    }
}

#[test]
fn streamed_payload_preserves_original_tuple_domain_and_charges_exact_bytes() {
    let mut transition = transition();
    for with_prior in [false, true] {
        if with_prior {
            transition.expected = Some(ProjectionBinding {
                projection: transition.projection,
                producer: transition.producer,
                owners: MapRoot(Some(ObjectRef(Digest([11; 32])))),
                source: transition.target_source,
                schema: transition.schema,
                producer_policy: transition.producer_policy,
                dependency_context: transition.dependency_context,
                certificate: ObjectRef(Digest([12; 32])),
            });
            transition.completeness = ProducerCompleteness::Partial {
                missing: MapRoot(Some(ObjectRef(Digest([13; 32])))),
            };
        }
        // The original pre-streaming implementation's exact tuple and domain.
        let encoded = serde_json::to_vec(&(
            transition.projection,
            transition.producer,
            &transition.expected,
            transition.target_source,
            transition.schema,
            transition.producer_policy,
            transition.dependency_context,
            transition.completeness,
            &transition.owners,
        ))
        .expect("original payload JSON");
        let domain = b"quanta-index-projection-transition-v1\0";
        let mut hash = Sha256::new();
        hash.update(domain);
        hash.update(&encoded);
        let expected = Digest(hash.finalize().into());
        let mut charged = 0_usize;
        let actual = transition
            .payload_digest_with_work_v3(&mut |bytes| {
                charged = charged.checked_add(bytes).expect("bounded test work");
                Ok::<(), &'static str>(())
            })
            .expect("streamed payload");
        assert_eq!(actual, expected);
        assert_eq!(
            transition.payload_digest().expect("ordinary facade"),
            expected
        );
        assert_eq!(
            charged,
            domain
                .len()
                .checked_add(encoded.len())
                .expect("test length")
        );

        transition.events.push(SourceEvent {
            producer: transition.producer,
            stream: StreamId([14; 16]),
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: expected,
        });
        assert_eq!(
            transition
                .payload_digest()
                .expect("event-independent payload"),
            expected
        );
    }
}

#[test]
fn streamed_payload_preserves_first_and_mid_json_typed_refusal() {
    let transition = transition();
    for allowed_calls in [0_usize, 3_usize] {
        let mut paid_calls = 0_usize;
        let cause = transition
            .payload_digest_with_work_v3(&mut |_| {
                if paid_calls == allowed_calls {
                    return Err("original cancelled");
                }
                paid_calls = paid_calls.checked_add(1).expect("bounded test calls");
                Ok(())
            })
            .expect_err("refuse before the next hash chunk");
        assert!(matches!(
            cause,
            ProjectionPayloadDigestErrorV3::Work("original cancelled")
        ));
        assert_eq!(paid_calls, allowed_calls);
    }
}
