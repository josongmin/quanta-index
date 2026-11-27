//! Capture profile metadata through the original retained read authority.

use super::*;
use quanta_index_contract::native_protocol::{
    MAX_NATIVE_READ_PROFILE_INSTANCES, NativeReadSessionProfile,
};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Does not acquire a second pin or advance Latest. The daemon authenticates
    /// the profile object using the same ticket as subsequent retained queries.
    pub fn read_session_profile(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionProfile, NativeUdsError> {
        self.read_session_profile_with_observation(expected, budget, None)
    }

    pub(crate) fn read_session_profile_with_observation(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
        observation: Option<&mut ClientLexicalQueryObservationV1>,
    ) -> Result<NativeReadSessionProfile, NativeUdsError> {
        validate_profile_request(expected)?;
        let NativeIndexResponse::ReadSessionProfile(result) = self.request_lexical(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
                session: expected.session,
            }),
            budget,
            observation,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained profile reply".into(),
            ));
        };
        validate_profile(expected, &result)?;
        Ok(result)
    }
}

pub(super) fn validate_profile_request(
    expected: &NativeReadSessionStatus,
) -> Result<(), NativeUdsError> {
    if expected.state != NativeReadSessionState::Active {
        return Err(NativeUdsError::Protocol(
            "profile read needs an active session".into(),
        ));
    }
    Ok(())
}

fn validate_profile(
    expected: &NativeReadSessionStatus,
    result: &NativeReadSessionProfile,
) -> Result<(), NativeUdsError> {
    validate_profile_with_memory(expected, result, None)
}

pub(super) fn validate_profile_with_memory(
    expected: &NativeReadSessionStatus,
    result: &NativeReadSessionProfile,
    memory: Option<&super::super::MemoryScope>,
) -> Result<(), NativeUdsError> {
    if result.session != expected.session || result.view.identity != expected.view {
        return Err(NativeUdsError::Protocol(
            "retained profile session or view differs".into(),
        ));
    }
    let digest = result
        .view
        .identity_digest()
        .map_err(|error| NativeUdsError::Protocol(format!("profile view digest: {error}")))?;
    if digest != result.view.identity.digest {
        return Err(NativeUdsError::Protocol(
            "retained profile view digest differs".into(),
        ));
    }
    if result.profile.instances.len() > MAX_NATIVE_READ_PROFILE_INSTANCES {
        return Err(NativeUdsError::Protocol(
            "retained profile exceeds instance bound".into(),
        ));
    }
    let _scratch = super::super::temporary_rows::<&quanta_index_contract::index::InstanceSpec>(
        memory,
        result.profile.instances.len(),
    )?;
    let mut instances = Vec::new();
    instances
        .try_reserve_exact(result.profile.instances.len())
        .map_err(|error| NativeUdsError::Transport(error.to_string()))?;
    instances.extend(result.profile.instances.iter());
    instances.sort_unstable_by_key(|spec| spec.instance);
    if instances
        .windows(2)
        .any(|pair| pair[0].instance == pair[1].instance)
    {
        return Err(NativeUdsError::Protocol(
            "retained profile has duplicate instance or mixed activation".into(),
        ));
    }
    instances.sort_unstable_by_key(|spec| spec.group);
    if instances
        .windows(2)
        .any(|pair| pair[0].group == pair[1].group && pair[0].activation != pair[1].activation)
    {
        return Err(NativeUdsError::Protocol(
            "retained profile has duplicate instance or mixed activation".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use quanta_index_contract::index::{
        CatalogView, CollectionId, Digest, GroupActivation, IndexKind, IndexProfile, InputDomain,
        InstanceSpec, MapRoot, ObjectRef, PublicationGroupId, SchemaId, SlotId, SourceCut,
        StorageFormatId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
    };

    pub(in crate::native_uds::read_session) fn fixture()
    -> Result<(NativeReadSessionStatus, NativeReadSessionProfile), Box<dyn std::error::Error>> {
        let mut view = CatalogView {
            identity: ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 2,
                sequence: 9,
                digest: Digest([0; 32]),
            },
            profile: ObjectRef(Digest([5; 32])),
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        view.identity.digest = view.identity_digest()?;
        let status = NativeReadSessionStatus {
            session: NativeReadSessionId([6; 32]),
            view: view.identity,
            expires_at_ms: 100,
            revision: 1,
            state: NativeReadSessionState::Active,
        };
        let profile = IndexProfile {
            instances: vec![InstanceSpec {
                instance: IndexInstanceId([7; 16]),
                kind: IndexKind([8; 16]),
                group: PublicationGroupId([9; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([10; 16]),
                policy: Digest([11; 32]),
                format: StorageFormatId([12; 16]),
            }],
        };
        Ok((
            status,
            NativeReadSessionProfile {
                session: status.session,
                view,
                profile,
            },
        ))
    }

    fn distinct_instances(seed: &InstanceSpec, count: u128) -> Vec<InstanceSpec> {
        (1..=count)
            .map(|identity| {
                let mut spec = seed.clone();
                spec.instance = IndexInstanceId(identity.to_be_bytes());
                spec
            })
            .collect()
    }

    #[cfg(unix)]
    fn exchange(
        status: &NativeReadSessionStatus,
        reply: NativeReadSessionProfile,
    ) -> Result<Result<NativeReadSessionProfile, NativeUdsError>, Box<dyn std::error::Error>> {
        use quanta_index_contract::native_protocol::{
            NativeIndexRequestEnvelope, NativeIndexResponseEnvelope,
        };
        use std::{
            io::Write,
            os::unix::net::UnixListener,
            time::{Duration, Instant},
        };
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("profile.sock");
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let expected_session = status.session;
        let server = std::thread::spawn(move || -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error.to_string()),
                }
            };
            // BSD accept inherits the listener's O_NONBLOCK flag.
            stream
                .set_nonblocking(false)
                .map_err(|error| error.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
            if !matches!(request.payload, NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile { session }) if session == expected_session)
            {
                return Err(
                    "profile consumer opened another view or changed retained identity".into(),
                );
            }
            let envelope = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::ReadSessionProfile(reply),
            };
            let encoded =
                quanta_index_ipc::encode_response(&envelope).map_err(|error| error.to_string())?;
            stream
                .write_all(&encoded)
                .map_err(|error| error.to_string())
        });
        let result = NativeUdsClient::new(&path).read_session_profile(
            status,
            &RequestBudgetV1::for_duration(Duration::from_secs(5)),
        );
        server.join().map_err(|_panic| "profile peer panicked")??;
        Ok(result)
    }

    #[cfg(unix)]
    #[test]
    fn retained_profile_uses_original_session_and_preserves_complete_spec()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, gold) = fixture()?;
        assert_eq!(exchange(&status, gold.clone())??, gold);
        // Fixed public upper bound: unique identities and uniform group
        // activation leave only the cardinality rule under test.
        let mut maximum = gold;
        maximum.profile.instances = distinct_instances(&maximum.profile.instances[0], 4096);
        assert_eq!(exchange(&status, maximum.clone())??, maximum);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn retained_profile_rejects_hostile_identity_digest_and_instance_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, gold) = fixture()?;
        let mut cases = Vec::new();
        let mut wrong = gold.clone();
        wrong.session = NativeReadSessionId([20; 32]);
        cases.push(wrong);
        let mut wrong = gold.clone();
        wrong.view.identity.epoch += 1;
        // A self-consistent foreign view must fail exact retained binding,
        // even when its own CatalogView digest is valid.
        wrong.view.identity.digest = wrong.view.identity_digest()?;
        cases.push(wrong);
        let mut wrong = gold.clone();
        wrong.view.identity.sequence += 1;
        // A self-consistent foreign view must fail exact retained binding,
        // even when its own CatalogView digest is valid.
        wrong.view.identity.digest = wrong.view.identity_digest()?;
        cases.push(wrong);
        let mut wrong = gold.clone();
        wrong.view.profile = ObjectRef(Digest([21; 32]));
        cases.push(wrong);
        let mut wrong = gold.clone();
        wrong
            .profile
            .instances
            .push(wrong.profile.instances[0].clone());
        cases.push(wrong);
        let mut wrong = gold.clone();
        let mut other = wrong.profile.instances[0].clone();
        other.instance = IndexInstanceId([22; 16]);
        other.activation = GroupActivation::AsyncCoverage;
        wrong.profile.instances.push(other);
        cases.push(wrong);
        let mut wrong = gold;
        // Keep every other structural rule valid: a duplicate-ID refusal
        // must not hide removal of the independent instance-count guard.
        wrong.profile.instances = distinct_instances(&wrong.profile.instances[0], 4097);
        cases.push(wrong);
        for (index, reply) in cases.into_iter().enumerate() {
            assert!(
                matches!(exchange(&status, reply)?, Err(NativeUdsError::Protocol(_))),
                "hostile profile case {index} escaped SDK validation"
            );
        }
        Ok(())
    }

    #[test]
    fn retained_profile_rejects_inactive_status_before_socket_io()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut status, _) = fixture()?;
        let directory = tempfile::tempdir()?;
        let socket = directory.path().join("absent.sock");
        let client = NativeUdsClient::new(&socket);
        for state in [
            NativeReadSessionState::Closing,
            NativeReadSessionState::Released,
            NativeReadSessionState::Unknown,
        ] {
            status.state = state;
            assert!(matches!(
                client.read_session_profile(
                    &status,
                    &RequestBudgetV1::for_duration(std::time::Duration::from_secs(1))
                ),
                Err(NativeUdsError::Protocol(_))
            ));
        }
        Ok(())
    }
}
