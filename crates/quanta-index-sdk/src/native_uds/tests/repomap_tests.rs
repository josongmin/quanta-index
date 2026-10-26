use super::*;
use quanta_index_contract::{
    FileId, RepoMapNodeRef, RepoRelativePath, SymbolId,
    index::{Digest, IndexInstanceId, ViewRef, ViewSelection},
    native_protocol::{
        NativeIndexResponse, NativeRepoMapHit, NativeRepoMapQuery, NativeRepoMapResult,
        native_repomap_rank_policy,
    },
};

fn rank_view() -> ViewRef {
    ViewRef {
        workspace: key(1).workspace,
        epoch: 1,
        sequence: 7,
        digest: Digest([61; 32]),
    }
}

fn rank_query() -> NativeRepoMapQuery {
    NativeRepoMapQuery {
        query_text: "alpha".into(),
        focus_subjects: Vec::new(),
        top_k: 2,
        token_budget: 128,
    }
}

fn rank_hit(path: &str, rank: u32) -> NativeRepoMapHit {
    NativeRepoMapHit {
        subject: RepoMapNodeRef::File(FileId::new(path)),
        owner_path: RepoRelativePath::new(path),
        final_score_millis: 900,
        rank,
    }
}

fn rank_result() -> NativeRepoMapResult {
    NativeRepoMapResult {
        view: rank_view(),
        instance: IndexInstanceId([62; 16]),
        policy: native_repomap_rank_policy(),
        hits: vec![rank_hit("src/a.rs", 1)],
        dropped_entries_count: 0,
        drop_reason_codes: Vec::new(),
        degraded_reason_codes: Vec::new(),
        exhaustive: true,
    }
}

fn rank_reply_for(
    selection: ViewSelection,
    query: NativeRepoMapQuery,
    response: NativeIndexResponse,
) -> Result<NativeRepoMapResult, NativeUdsError> {
    scripted_reply(response, |client, budget| {
        client.query_repomap(selection, IndexInstanceId([62; 16]), query, budget)
    })
}

#[test]
fn repomap_sdk_binds_exact_and_latest_reply() -> Result<(), Box<dyn std::error::Error>> {
    let result = rank_result();
    for selection in [
        ViewSelection::Exact(rank_view()),
        ViewSelection::Latest {
            workspace: rank_view().workspace,
            epoch: rank_view().epoch,
        },
    ] {
        let observed = rank_reply_for(
            selection,
            rank_query(),
            NativeIndexResponse::RepoMapRanked(result.clone()),
        )?;
        if observed != result {
            return Err("RepoMap SDK changed a selected rank result".into());
        }
    }
    Ok(())
}

#[test]
fn repomap_sdk_rejects_foreign_or_incomplete_result() -> Result<(), Box<dyn std::error::Error>> {
    let valid = rank_result();
    let cases = [
        NativeRepoMapResult {
            view: ViewRef {
                sequence: 8,
                ..valid.view
            },
            ..valid.clone()
        },
        NativeRepoMapResult {
            instance: IndexInstanceId([63; 16]),
            ..valid.clone()
        },
        NativeRepoMapResult {
            exhaustive: false,
            ..valid.clone()
        },
        NativeRepoMapResult {
            hits: vec![rank_hit("src/a.rs", 1), rank_hit("src/b.rs", 2)],
            ..valid.clone()
        },
        NativeRepoMapResult {
            hits: vec![rank_hit("src/a.rs", 1), rank_hit("src/a.rs", 2)],
            ..valid
        },
    ];
    for (index, result) in cases.into_iter().enumerate() {
        let mut query = rank_query();
        if index == 3 {
            query.top_k = 1;
        }
        if !matches!(
            rank_reply_for(
                ViewSelection::Exact(rank_view()),
                query,
                NativeIndexResponse::RepoMapRanked(result)
            ),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err("RepoMap SDK accepted a forged selected result".into());
        }
    }
    Ok(())
}

#[test]
fn repomap_sdk_rejects_cross_variant_raw_identity_collision()
-> Result<(), Box<dyn std::error::Error>> {
    let result = NativeRepoMapResult {
        hits: vec![
            rank_hit("src/a.rs", 1),
            NativeRepoMapHit {
                subject: RepoMapNodeRef::Symbol(SymbolId::new("src/a.rs")),
                ..rank_hit("src/a.rs", 2)
            },
        ],
        ..rank_result()
    };
    if !matches!(
        rank_reply_for(
            ViewSelection::Exact(rank_view()),
            rank_query(),
            NativeIndexResponse::RepoMapRanked(result)
        ),
        Err(NativeUdsError::Protocol(_))
    ) {
        return Err("RepoMap SDK accepted File/Symbol raw-ID collision".into());
    }
    Ok(())
}

#[test]
fn repomap_sdk_rejects_wrong_variant_and_invalid_request() -> Result<(), Box<dyn std::error::Error>>
{
    if !matches!(
        rank_reply_for(
            ViewSelection::Exact(rank_view()),
            rank_query(),
            NativeIndexResponse::Created(rank_view())
        ),
        Err(NativeUdsError::Protocol(_))
    ) {
        return Err("RepoMap SDK accepted an unrelated response variant".into());
    }
    let client = NativeUdsClient::new(std::path::PathBuf::from("/no/native/socket"));
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(1));
    let mut invalid = rank_query();
    invalid.top_k = 0;
    if !matches!(
        client.query_repomap(
            ViewSelection::Exact(rank_view()),
            IndexInstanceId([62; 16]),
            invalid,
            &budget
        ),
        Err(NativeUdsError::Protocol(_))
    ) {
        return Err("RepoMap SDK connected for invalid request".into());
    }
    Ok(())
}
