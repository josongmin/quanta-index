//! A repository revision sealed as a protected Source head, for `.text()`.
//!
//! The service is composed like the product: the Text reader names the
//! Literal storage format as its sibling, and the registry requires exactly
//! one Literal instance beside every Text instance. A primary view and the
//! revision's own workspace are published, then `(repo, revision)` is sealed
//! as a protected Source head, which the SDK `.text()` route resolves.

use super::*;
use quanta_index_contract::index::{
    NativeProtectedSourceHeadV1, NativeProtectedSourceRoutesV1,
    native_protected_view_marker_bytes_v1,
};
use quanta_index_contract::native_protocol::{
    NativeImmutableObjectChildrenV1, NativeImmutableObjectPayloadV1,
};
use quanta_index_contract::{RepoId, RevisionId};

const STAGE_BATCH: usize = 64;

#[derive(Clone, Copy)]
pub(super) enum AnchoredTextLayout {
    Tree,
    SegmentedLayeredV2,
}

impl AnchoredTextLayout {
    fn recipe(self) -> TextRecipe {
        match self {
            Self::Tree => TextRecipe::new(Digest([31; 32])),
            Self::SegmentedLayeredV2 => TextRecipe::segmented_layered(Digest([31; 32])),
        }
    }

    pub(super) fn text_spec(self) -> InstanceSpec {
        let mut spec = native_text_spec();
        if matches!(self, Self::SegmentedLayeredV2) {
            spec.policy = self.recipe().output_policy();
            spec.format = StorageFormatId([77; 16]);
        }
        spec
    }
}

/// One revision: its identity, its own workspace slot and its files.
pub(super) struct ProtectedRevision<'a> {
    pub(super) repo: &'a str,
    pub(super) revision: &'a str,
    pub(super) slot: u8,
    /// Leading byte of every file owner's stable id.
    pub(super) owner_tag: u8,
}

impl ProtectedRevision<'_> {
    pub(super) fn source_scope(&self) -> WorkspaceKey {
        WorkspaceKey {
            slot: SlotId([self.slot; 16]),
            ..workspace()
        }
    }

    fn in_source_scope(&self, sequence: u64) -> OperationKey {
        let mut key = operation(sequence);
        key.workspace = self.source_scope();
        key
    }
}

fn literal_descriptor() -> AdapterDescriptor {
    let spec = literal_spec();
    AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    }
}

fn anchored_profile(layout: AnchoredTextLayout) -> IndexProfile {
    IndexProfile {
        instances: vec![literal_spec(), layout.text_spec()],
    }
}

/// The product wiring: Literal-sibling reader plus the companion rule.
pub(super) fn anchored_service(
    root: &std::path::Path,
    socket: &std::path::Path,
) -> TestResult<NativeIndexService> {
    anchored_service_with_layout(root, socket, AnchoredTextLayout::Tree)
}

pub(super) fn anchored_service_with_layout(
    root: &std::path::Path,
    socket: &std::path::Path,
    layout: AnchoredTextLayout,
) -> TestResult<NativeIndexService> {
    let resources = read_session_resources(Duration::from_secs(60));
    let recipe = layout.recipe();
    let spec = layout.text_spec();
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let mut capabilities = reclaim_capabilities(None)?;
    capabilities
        .builders
        .register(descriptor, NativeTextIndex::new(recipe), |delta| {
            Ok(delta.clone())
        })?;
    capabilities
        .builders
        .require_companion(descriptor.kind, literal_descriptor())?;
    let read_need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    capabilities.text.register(
        descriptor,
        read_need,
        NativeTextReader::new(recipe, Bm25Policy { k1: 1.2, b: 0.75 }, read_need)?
            .with_literal_sibling(literal_spec().format),
    )?;
    let service = NativeIndexService::bind_with_retention(
        root,
        socket,
        work_limits(),
        capabilities,
        &resources,
        Some(test_reclaim_config()),
        None,
    )?;
    service.configure_read_sessions(NativeReadSessionConfig::default())?;
    Ok(service)
}

fn configure<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    scope: WorkspaceKey,
    key: OperationKey,
    layout: AnchoredTextLayout,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let NativeIndexResponse::Created(genesis) = client
        .request(
            NativeIndexRequest::CreateWorkspace {
                workspace: scope,
                epoch: 1,
            },
            &budget(),
        )
        .map_err(|error| format!("create protected revision workspace: {error}"))?
    else {
        return Err("revision workspace was not created".into());
    };
    let ingress = client
        .begin_input(key, &budget())
        .map_err(|error| format!("begin protected revision profile: {error}"))?;
    let profile = client
        .stage_profile(ingress, anchored_profile(layout), &budget())
        .map_err(|error| format!("stage protected revision profile: {error}"))?;
    published(
        &client
            .submit(
                UpdateRequest {
                    operation: key,
                    expected_view: genesis,
                    kind: UpdateKind::Reconfigure { profile },
                },
                &budget(),
            )
            .map_err(|error| format!("submit protected revision profile: {error}"))?,
    )
}

/// Publish all files as one repository revision; returns the Source refs
/// in file order.
fn publish_source_revision<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    revision: &ProtectedRevision<'_>,
    files: &[(String, Vec<u8>)],
    files_per_publication: usize,
    layout: AnchoredTextLayout,
) -> TestResult<(OperationKey, Vec<ObjectRef>)> {
    if files_per_publication == 0 || files.is_empty() {
        return Err("revision fixture requires files and a nonzero publication batch".into());
    }
    let mut view = configure(
        client,
        revision.source_scope(),
        revision.in_source_scope(1),
        layout,
    )?;
    let mut sources = Vec::with_capacity(files.len());
    let mut last_operation = None;
    let repository = RepoId::new(revision.repo)?;
    for (page, files) in files.chunks(files_per_publication).enumerate() {
        let sequence = u64::try_from(page)?
            .checked_add(2)
            .ok_or("publication sequence overflow")?;
        let operation = revision.in_source_scope(sequence);
        let ingress = client
            .begin_input(operation, &budget())
            .map_err(|error| format!("open protected revision operation {sequence}: {error}"))?;
        let start = sources.len();
        for (batch_index, batch) in files.chunks(STAGE_BATCH).enumerate() {
            let inputs = batch
                .iter()
                .map(|(path, bytes)| SourceInput {
                    path: path.clone(),
                    bytes: bytes.clone(),
                    text: SourceTextAdmission::Utf8 {
                        policy: Digest([31; 32]),
                    },
                    language: None,
                })
                .collect();
            let NativeIndexResponse::StagedSources(staged) = client.request(
                NativeIndexRequest::StageRepositorySources {
                    ingress,
                    repository: repository.clone(),
                    inputs,
                },
                &budget(),
            ).map_err(|error| {
                format!(
                    "stage Source bodies for protected revision operation {sequence}, publication page {page}, stage batch {batch_index} ({} files): {error}",
                    batch.len()
                )
            })?
            else {
                return Err("revision sources were not staged".into());
            };
            if staged.len() != batch.len() {
                return Err("revision staging lost a source".into());
            }
            sources.extend(staged);
        }
        let mutations = sources
            .iter()
            .enumerate()
            .skip(start)
            .map(|(ordinal, source)| -> TestResult<OwnerMutation> {
                let [high, low] = u16::try_from(ordinal)?.to_be_bytes();
                let mut stable = [revision.owner_tag; 16];
                stable[1] = high;
                stable[2] = low;
                Ok(OwnerMutation {
                    owner: OwnerKey {
                        producer: ingress.operation.producer,
                        stable_id: StableOwnerId(stable),
                    },
                    expected: None,
                    next: Some(*source),
                })
            })
            .collect::<TestResult<Vec<_>>>()?;
        // Mutation pages share one SDK deadline, including the final seal.
        let staging = RequestBudgetV1::for_duration(Duration::from_secs(120));
        let request = prepare_text_mutations_in_ingress_with_budget(
            client, view, ingress, mutations, &staging,
        )
        .map_err(|error| {
            format!(
                "prepare revision operation {sequence} ({} rows): {error}",
                files.len()
            )
        })?;
        // All reconciliations of this operation share one overall deadline.
        let publication = RequestBudgetV1::for_duration(Duration::from_secs(300));
        view =
            crate::publication_settle::submit_and_settle(client, &request, || publication.clone())
                .map_err(|error| format!("publish revision operation {sequence}: {error}"))?;
        last_operation = Some(operation);
    }
    Ok((last_operation.ok_or("revision was not published")?, sources))
}

fn stage_object<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    ingress: IngressRef,
    bytes: Vec<u8>,
    children: Vec<ObjectRef>,
) -> TestResult<ObjectRef> {
    let NativeIndexResponse::Staged(reference) = client.request(
        NativeIndexRequest::StageImmutableObject {
            ingress,
            bytes: NativeImmutableObjectPayloadV1::new(bytes)?,
            children: NativeImmutableObjectChildrenV1::new(children)?,
        },
        &budget(),
    )?
    else {
        return Err("protected route object was not staged".into());
    };
    Ok(reference)
}

/// Publish a primary view and the revision, then seal the revision as a
/// protected Source head. Returns the revision's Source refs in file order.
pub(super) fn publish_and_seal(
    root: &std::path::Path,
    socket: &std::path::Path,
    revision: &ProtectedRevision<'_>,
    files: &[(String, Vec<u8>)],
) -> TestResult<Vec<ObjectRef>> {
    publish_and_seal_batches(
        root,
        socket,
        revision,
        files,
        files.len().max(1),
        AnchoredTextLayout::Tree,
    )
}

pub(super) fn publish_and_seal_batches(
    root: &std::path::Path,
    socket: &std::path::Path,
    revision: &ProtectedRevision<'_>,
    files: &[(String, Vec<u8>)],
    files_per_publication: usize,
    layout: AnchoredTextLayout,
) -> TestResult<Vec<ObjectRef>> {
    let running = NativeServiceGuard::start(anchored_service_with_layout(root, socket, layout)?);
    let published_sources = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket);
        let NativeIndexResponse::Created(genesis) = client
            .request(
                NativeIndexRequest::CreateWorkspace {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )
            .map_err(|error| {
                format!("create primary workspace before protected revision: {error}")
            })?
        else {
            return Err("primary workspace was not created".into());
        };
        let key = operation(1);
        let ingress = client
            .begin_input(key, &budget())
            .map_err(|error| format!("begin primary profile before protected revision: {error}"))?;
        let profile = client
            .stage_profile(ingress, anchored_profile(layout), &budget())
            .map_err(|error| format!("stage primary profile before protected revision: {error}"))?;
        let primary = published(
            &client
                .submit(
                    UpdateRequest {
                        operation: key,
                        expected_view: genesis,
                        kind: UpdateKind::Reconfigure { profile },
                    },
                    &budget(),
                )
                .map_err(|error| {
                    format!("submit primary profile before protected revision: {error}")
                })?,
        )?;
        let _primary = publish_text_owner(&client, primary, 2, 1, "primary.rs", "primary only")
            .map_err(|error| {
                format!("publish primary Source before protected revision: {error}")
            })?;
        publish_source_revision(&client, revision, files, files_per_publication, layout)
            .map_err(|error| format!("publish protected Source revision: {error}").into())
    })();
    let stopped = running.stop();
    let (source_key, sources) = published_sources?;
    stopped?;

    let catalog = SqliteCatalog::open(root, Duration::from_secs(2))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let source_operation = catalog.operation(source_key)?;
    let publication = catalog.published_view_metadata(source_key, &mut work)?;
    let routes = NativeProtectedSourceRoutesV1 {
        wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
        native_origin: None,
        heads: vec![NativeProtectedSourceHeadV1 {
            repo_id: RepoId::new(revision.repo)?,
            revision_id: RevisionId::new(revision.revision)?,
            source_operation,
            source_publication: publication.clone(),
            operation: source_operation,
            publication: publication.clone(),
        }],
        repomap_heads: Vec::new(),
    };
    routes.validate()?;
    drop(catalog);

    let running = NativeServiceGuard::start(anchored_service_with_layout(root, socket, layout)?);
    let sealed = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let ingress = client
            .begin_input(operation(50), &budget())
            .map_err(|error| format!("open protected route seal: {error}"))?;
        let _marker = stage_object(
            &client,
            ingress,
            native_protected_view_marker_bytes_v1(publication.identity)?,
            Vec::new(),
        )
        .map_err(|error| format!("stage protected view marker: {error}"))?;
        let routes_ref = stage_object(
            &client,
            ingress,
            serde_json::to_vec(&routes)?,
            routes.children()?,
        )
        .map_err(|error| format!("stage protected Source routes: {error}"))?;
        let NativeIndexResponse::RootScopedView(_selected) = client
            .request(
                NativeIndexRequest::SealCandidateWithSourceRoutes {
                    primary: operation(2),
                    ingress,
                    routes: routes_ref,
                },
                &budget(),
            )
            .map_err(|error| {
                format!(
                    "seal protected Source routes for {} files: {error}",
                    files.len()
                )
            })?
        else {
            return Err("protected Source routes were not sealed".into());
        };
        Ok(())
    })();
    let stopped = running.stop();
    sealed?;
    stopped?;
    Ok(sources)
}
