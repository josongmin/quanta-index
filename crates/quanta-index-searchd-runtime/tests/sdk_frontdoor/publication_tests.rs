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

#[test]
fn sdk_default_code_search_matches_terms_across_chunks_as_one_file() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;
    let corpus_batch = lexical_batch()?;
    let _active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;

    let response = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .text("todo main")
                .active(repo(), revision())
                .top_k(10)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let file = response.results.first().ok_or("missing file result")?;
    let scope = corpus_batch
        .replace_scopes()
        .first()
        .ok_or("missing replacement scope")?;
    if file.repo_relative_path.as_str() != "src/lib.rs"
        || !file.candidate_id.starts_with("file:")
        || file.source.as_ref().map(|source| source.source_sha256)
            != Some(scope.coverage.source.source_sha256)
    {
        return Err(format!("default CodeSearch file identity drift: {file:?}").into());
    }

    let two_files = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .text("sphinx")
                .active(repo(), revision())
                .top_k(10)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let paths = two_files
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str())
        .collect::<BTreeSet<_>>();
    if paths != BTreeSet::from(["src/alpha.rs", "src/beta.rs"]) {
        return Err(format!("CodeSearch file ranking returned {paths:?}").into());
    }

    let first_page = client
        .lexical()
        .query()
        .text("sphinx")
        .active(repo(), revision())
        .top_k(1)
        .execute()?;
    if first_page.rank_unit != quanta_index_contract::TextRankUnit::File
        || first_page.results.len() != 1
    {
        return Err(format!("CodeSearch first file page drift: {first_page:?}").into());
    }
    let cursor = first_page
        .next_cursor
        .ok_or("CodeSearch first file page lacks continuation")?;
    let second_page = client
        .lexical()
        .query()
        .text("sphinx")
        .pinned(first_page.generation.clone())
        .top_k(1)
        .after(cursor)
        .execute()?;
    let first_file = first_page
        .results
        .first()
        .ok_or("missing first file page result")?;
    let second_file = second_page
        .results
        .first()
        .ok_or("missing second file page result")?;
    if second_page.rank_unit != quanta_index_contract::TextRankUnit::File
        || second_page.results.len() != 1
        || second_page.next_cursor.is_some()
        || first_file.repo_relative_path == second_file.repo_relative_path
    {
        return Err(format!("CodeSearch second file page drift: {second_page:?}").into());
    }

    for (query, expected_path) in [
        ("regex:/sphinx.*quartz/", "src/alpha.rs"),
        ("content:regex:/sphinx.*riddles/", "src/beta.rs"),
        ("path:regex:/beta[.]rs/", "src/beta.rs"),
        ("sphinx regex:/quartz/", "src/alpha.rs"),
    ] {
        let response = client
            .lexical()
            .query()
            .text(query)
            .active(repo(), revision())
            .top_k(10)
            .execute()?;
        let actual = response
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str())
            .collect::<Vec<_>>();
        if actual != [expected_path] {
            return Err(format!("CodeSearch regex {query:?} returned {actual:?}").into());
        }
        if query == "path:regex:/beta[.]rs/" {
            let hit = response.results.first().ok_or("missing path hit")?;
            if hit.preview.as_ref().map(|preview| preview.kind)
                != Some(quanta_index_contract::PreviewKind::Path)
                || hit.snippet != "src/beta.rs"
                || hit.snippet_hit_offset != Some(4)
                || hit.highlights != [quanta_index_contract::HighlightSpan { start: 4, len: 7 }]
            {
                return Err(format!("CodeSearch path match position drift: {hit:?}").into());
            }
        }
    }
    fixture.stop()
}
