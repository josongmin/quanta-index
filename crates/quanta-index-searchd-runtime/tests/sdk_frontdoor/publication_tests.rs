use super::*;

#[test]
fn sdk_publish_frontdoor_routes_ingest_batches() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let lexical_receipt = client.search_corpus().publish(&lexical_batch()?)?;
    let history_receipt = client.history().publish(&history_batch())?;
    let dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let structural_receipt = client.structural().publish(&structural_batch()?)?;
    let repo_map_receipt = client
        .repomap()
        .publish(&RepoMapPublishBundleRequestV2::new(repo_map_bundle()?)?)?;

    if lexical_receipt.generation != generation()
        || lexical_receipt.accepted_replace_scopes != 3
        || lexical_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected search-corpus receipt: {lexical_receipt:?}").into());
    }
    if history_receipt.generation != generation()
        || history_receipt.accepted_replace_scopes != 4
        || history_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected history receipt: {history_receipt:?}").into());
    }
    if dirty_receipt.generation != generation()
        || dirty_receipt.accepted_replace_scopes != 1
        || dirty_receipt.accepted_tombstone_scopes != 1
    {
        return Err(format!("unexpected dirty receipt: {dirty_receipt:?}").into());
    }
    if structural_receipt.generation != generation()
        || structural_receipt.accepted_replace_scopes != 1
        || structural_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected structural receipt: {structural_receipt:?}").into());
    }
    if repo_map_receipt.mutation.repo_id != repo()
        || repo_map_receipt.mutation.revision_id != revision()
        || repo_map_receipt.mutation.manifest_generation != generation()
    {
        return Err(format!("unexpected repo-map receipt: {repo_map_receipt:?}").into());
    }

    fixture.stop()
}

#[test]
fn sdk_repomap_active_head_tracks_only_catalog_activation() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    if client.repomap().active_head(repo(), revision())?.is_some() {
        return Err("fresh RepoMap catalog unexpectedly has an active head".into());
    }
    let publish = client
        .repomap()
        .publish(&RepoMapPublishBundleRequestV2::new(repo_map_bundle()?)?)?;
    if client.repomap().active_head(repo(), revision())?.is_some() {
        return Err("RepoMap publish changed the active head".into());
    }
    let activation = client.repomap().activate(repo_map_activate_request()?)?;
    let head = client
        .repomap()
        .active_head(repo(), revision())?
        .ok_or("RepoMap activation has no catalog head")?;
    if head.epoch().get() != activation.mutation.activation_epoch
        || head.candidate_commitment().to_wire_string()
            != activation.mutation.new_candidate_commitment
        || activation.mutation.new_candidate_commitment != publish.mutation.new_candidate_commitment
    {
        return Err(format!("RepoMap catalog head diverges from receipts: {head:?}").into());
    }

    fixture.stop()
}
