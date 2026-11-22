//! A sealed candidate may reopen only while its exact protected BOOT is current.

use super::*;
use crate::native::prepare_native_boot_authority;
use quanta_index_contract::index::{
    NativeProtectedSourceHeadV1, NativeProtectedSourceRoutesV1,
    native_protected_source_route_seal_key_v1, native_protected_view_marker_bytes_v1,
    native_protected_view_marker_ref_v1,
};
use quanta_index_contract::native_protocol::{
    NativeImmutableObjectChildrenV1, NativeImmutableObjectPayloadV1,
};
use quanta_index_contract::{RepoId, RevisionId};
use quanta_index_searchd::app::runtime::StateRootLease;

fn candidate_guard(fixture: &PreparedNativeFixture) -> Result<()> {
    let lease = Arc::new(StateRootLease::acquire_existing(&fixture.root)?);
    let authority = prepare_native_boot_authority(
        lease,
        &fixture.config.socket_path,
        &fixture.config.resources,
        &request(),
    )?;
    let registry = compose_boot_registry(&fixture.config)?;
    boot::require_candidate_start_v1(&authority, &fixture.config, &registry, &request())?;
    boot::require_retired(&authority)
}

fn start_candidate(fixture: &mut PreparedNativeFixture, name: &str) -> TestResult {
    let service = bind(
        &fixture.config,
        fixture.config_digest,
        compose(&fixture.config)?,
        None,
        &request(),
        true,
    )?;
    let shutdown = service.shutdown_handle();
    fixture.running = RunningFixture {
        shutdown,
        thread: Some(
            thread::Builder::new()
                .name(name.into())
                .spawn(move || service.run(Duration::from_millis(10)))?,
        ),
    };
    Ok(())
}

fn route_seal_key(fixture: &PreparedNativeFixture) -> TestResult<OperationKey> {
    let NativeIndexResponse::RootScopedView(root) = fixture.client.request(
        NativeIndexRequest::ObserveRootView(ViewSelection::Exact(fixture.native_view)),
        &request(),
    )?
    else {
        return Err("candidate root identity was not observed".into());
    };
    Ok(native_protected_source_route_seal_key_v1(
        operation(2),
        root.root_nonce,
    )?)
}

fn stage_immutable(
    client: &NativeUdsClient,
    ingress: quanta_index_contract::index::IngressRef,
    bytes: Vec<u8>,
    children: Vec<quanta_index_contract::index::ObjectRef>,
) -> TestResult<quanta_index_contract::index::ObjectRef> {
    let NativeIndexResponse::Staged(reference) = client.request(
        NativeIndexRequest::StageImmutableObject {
            ingress,
            bytes: NativeImmutableObjectPayloadV1::new(bytes)?,
            children: NativeImmutableObjectChildrenV1::new(children)?,
        },
        &request(),
    )?
    else {
        return Err("candidate immutable object was not staged".into());
    };
    Ok(reference)
}

fn sealed_candidate_fixture() -> TestResult<(
    PreparedNativeFixture,
    quanta_index_contract::index::ObjectRef,
)> {
    let mut fixture = prepare_native_fixture()?;
    fixture.running.stop()?;
    let published =
        boot::with_admitted_control_budget(&fixture.config, request(), false, |budget, _| {
            boot::with_boot_authority(&fixture.config, budget, |authority, budget| {
                let result = quanta_index_catalog::verify_native_published_operation(
                    &fixture.root,
                    authority.io.as_ref(),
                    operation(2),
                    budget,
                )?;
                let published = (
                    result.operation().clone(),
                    result.publication_metadata().clone(),
                );
                boot::require_retired(&authority)?;
                Ok(published)
            })
        })?;
    start_candidate(&mut fixture, "candidate-resume-seal")?;
    let route = NativeProtectedSourceRoutesV1 {
        wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
        native_origin: None,
        heads: vec![NativeProtectedSourceHeadV1 {
            repo_id: RepoId::new("candidate-resume")?,
            revision_id: RevisionId::new("revision-resume")?,
            source_operation: published.0,
            source_publication: published.1.clone(),
            operation: published.0,
            publication: published.1,
        }],
        repomap_heads: Vec::new(),
    };
    route.validate()?;
    let ingress = fixture
        .client
        .begin_input(route_seal_key(&fixture)?, &request())?;
    let marker = stage_immutable(
        &fixture.client,
        ingress,
        native_protected_view_marker_bytes_v1(fixture.native_view)?,
        Vec::new(),
    )?;
    require(
        marker == native_protected_view_marker_ref_v1(fixture.native_view)?,
        "candidate marker differs from exact view",
    )?;
    let route_ref = stage_immutable(
        &fixture.client,
        ingress,
        serde_json::to_vec(&route)?,
        route.children()?,
    )?;
    require(
        route_ref == route.object_ref()?,
        "candidate route identity differs",
    )?;
    let NativeIndexResponse::RootScopedView(sealed) = fixture.client.request(
        NativeIndexRequest::SealCandidateWithSourceRoutes {
            primary: operation(2),
            ingress,
            routes: route_ref,
        },
        &request(),
    )?
    else {
        return Err("candidate protected BOOT was not sealed".into());
    };
    require(
        sealed.view.identity == fixture.native_view,
        "candidate BOOT changed head",
    )?;
    fixture.running.stop()?;
    Ok((fixture, route_ref))
}

#[test]
fn sealed_candidate_reopens_only_while_its_route_is_current() -> TestResult {
    let (mut fixture, _route_ref) = sealed_candidate_fixture()?;
    candidate_guard(&fixture)?;
    start_candidate(&mut fixture, "candidate-resume-stale")?;
    let (_later, _record) = super::capture_tests::advance_source(&fixture)?;
    fixture.running.stop()?;
    let stale = candidate_guard(&fixture).expect_err("stale protected route reopened");
    require(
        matches!(
            stale.downcast_ref::<IndexError>(),
            Some(IndexError::Conflict)
        ),
        "stale protected route did not refuse with Conflict",
    )?;
    Ok(())
}

#[test]
fn sealed_candidate_resumes_only_its_interrupted_route_ingress() -> TestResult {
    let (mut fixture, _route_ref) = sealed_candidate_fixture()?;
    start_candidate(&mut fixture, "candidate-resume-first-retry")?;
    let seal_key = route_seal_key(&fixture)?;
    let interrupted = fixture.client.begin_input(seal_key, &request())?;
    let _staged = stage_immutable(
        &fixture.client,
        interrupted,
        native_protected_view_marker_bytes_v1(fixture.native_view)?,
        Vec::new(),
    )?;
    fixture.running.stop()?;

    start_candidate(&mut fixture, "candidate-resume-second-retry")?;
    let reopened = fixture.client.begin_input(seal_key, &request())?;
    require(
        reopened.fence != interrupted.fence,
        "retry reused the interrupted fence",
    )?;
    let _pending = fixture.client.begin_input(operation(60), &request())?;
    fixture.running.stop()?;
    let pending = candidate_guard(&fixture).expect_err("unrelated pending work was accepted");
    require(
        matches!(
            pending.downcast_ref::<IndexError>(),
            Some(IndexError::Conflict)
        ),
        "pending work did not refuse with Conflict",
    )?;
    Ok(())
}

#[test]
fn missing_selected_route_object_refuses_candidate_resume() -> TestResult {
    let (fixture, route_ref) = sealed_candidate_fixture()?;
    let bytes = (route_ref.0).0;
    let name = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let path = fixture
        .root
        .join("native-objects")
        .join("objects")
        .join(format!("{:02x}", bytes[0]))
        .join(name);
    fs::remove_file(path)?;
    require(
        candidate_guard(&fixture).is_err(),
        "missing selected route reopened",
    )?;
    Ok(())
}

#[test]
fn routeless_selected_boot_is_not_a_candidate_resume() -> TestResult {
    let mut fixture = prepare_native_fixture()?;
    fixture.running.stop()?;
    let registry = compose_boot_registry(&fixture.config)?;
    let select = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: fixture.config_path.clone(),
        identity: None,
        config_digest: Some(fixture.config_digest),
        install_catalog_digest: None,
        select_operation: Some(operation(2)),
        receipt_path: Some(fixture.receipt_path.clone()),
    };
    require(
        boot::execute(&select, &fixture.config, &registry, request())?.is_none(),
        "routeless BOOT returned a service authority",
    )?;
    let routeless = candidate_guard(&fixture).expect_err("routeless BOOT reopened as candidate");
    require(
        matches!(
            routeless.downcast_ref::<IndexError>(),
            Some(IndexError::Conflict)
        ),
        "routeless BOOT did not refuse with Conflict",
    )?;
    Ok(())
}
