//! Local DELTA published between pages of an already pinned view.

use super::*;
use quanta_index_contract::{
    index::ViewRef,
    native_protocol::{NativeReadSessionRequest, NativeTextCursor},
};
use quanta_index_sdk::native_uds::NativeUdsError;

pub(super) struct Successor {
    pub(super) operation: OperationKey,
    pub(super) view: ViewRef,
    pub(super) fixture: RestartFixture,
    pub(super) hashes: BTreeMap<String, [u8; 32]>,
    pub(super) owners: BTreeMap<String, OwnerKey>,
    pub(super) refs: BTreeMap<String, ObjectRef>,
}

fn path(file: &ScopedFile) -> String {
    format!("{}/{}", file.source_repo_id, file.repo_relative_path)
}

pub(super) fn interleave(
    client: &NativeUdsClient,
    daemon: &Daemon,
    old_view: ViewRef,
    fixture: &RestartFixture,
    old_owners: &BTreeMap<String, OwnerKey>,
    old_refs: &BTreeMap<String, ObjectRef>,
) -> TestResult<Successor> {
    let mut next = None;
    let _old_rank = global_page::observe_with_first_page(
        client,
        old_view,
        fixture,
        old_owners,
        old_refs,
        [0x45; 32],
        Some(|old_cursor| {
            let successor = publish(client, daemon, old_view, fixture, old_owners, old_refs)?;
            check_foreign_cursor(client, &successor, old_cursor)?;
            next = Some(successor);
            Ok(())
        }),
    )?;
    next.ok_or_else(|| "local DELTA was not published between pages".into())
}

fn publish(
    client: &NativeUdsClient,
    daemon: &Daemon,
    old_view: ViewRef,
    fixture: &RestartFixture,
    old_owners: &BTreeMap<String, OwnerKey>,
    old_refs: &BTreeMap<String, ObjectRef>,
) -> TestResult<Successor> {
    let [_, _, deleted, renamed, _, replaced, ..] = fixture.files.as_slice() else {
        return Err("local DELTA fixture is too small".into());
    };
    let mut renamed_next = renamed.clone();
    renamed_next.repo_relative_path = "src/renamed.rs".into();
    let mut replaced_next = replaced.clone();
    replaced_next.content = format!("{} marker replacement", repo_query_token(1));
    let repeated_term = format!("{} ", global_page::QUERY).repeat(6);
    let added = ScopedFile {
        source_repo_id: "repo0".into(),
        repo_relative_path: "src/inserted.rs".into(),
        content: format!("{} {repeated_term}", repo_query_token(0)),
    };
    let changes = [
        (Some(deleted), None),
        (Some(renamed), Some(renamed_next)),
        (Some(replaced), Some(replaced_next)),
        (None, Some(added)),
    ];
    let operation = operation(3);
    let ingress = client.begin_input(operation, &fixture.budget())?;
    let staged_paths: Vec<_> = changes
        .iter()
        .filter_map(|(_, next)| next.as_ref())
        .map(path)
        .collect();
    let inputs: Vec<_> = changes
        .iter()
        .filter_map(|(_, next)| next.as_ref())
        .zip(&staged_paths)
        .map(|(file, path)| BorrowedSourceInput {
            path,
            bytes: file.content.as_bytes(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            language: None,
        })
        .collect();
    let refs = client.stage_sources_borrowed(ingress, &inputs, &fixture.budget())?;
    require_equal(&refs.len(), &inputs.len(), "DELTA staged replacements")?;
    let mut staged = refs.into_iter();
    let mut mutations = Vec::with_capacity(changes.len());
    let mut next_owners = old_owners.clone();
    let mut next_refs = old_refs.clone();
    let mut next_fixture = fixture.clone();
    for (prior, next) in changes {
        let (owner, expected) = if let Some(prior) = prior {
            let old_path = path(prior);
            let owner = next_owners.remove(&old_path).ok_or("missing prior owner")?;
            let reference = next_refs
                .remove(&old_path)
                .ok_or("missing prior reference")?;
            next_fixture.files.retain(|file| path(file) != old_path);
            (owner, Some(reference))
        } else {
            (
                OwnerKey {
                    producer: operation.producer,
                    stable_id: StableOwnerId(11_u128.to_be_bytes()),
                },
                None,
            )
        };
        let next = next
            .map(|next| -> TestResult<_> {
                let new_path = path(&next);
                let reference = staged.next().ok_or("missing staged DELTA reference")?;
                require(
                    Some(reference) != expected,
                    "DELTA rename or replacement reused its prior Source record",
                )?;
                require(
                    next_refs.insert(new_path.clone(), reference).is_none(),
                    "DELTA duplicate path",
                )?;
                require(
                    next_owners.insert(new_path, owner).is_none(),
                    "DELTA duplicate owner path",
                )?;
                next_fixture.files.push(next);
                Ok(reference)
            })
            .transpose()?;
        mutations.push(OwnerMutation {
            owner,
            expected,
            next,
        });
    }
    require(staged.next().is_none(), "unused staged DELTA source")?;
    require_equal(
        &next_fixture.files.len(),
        &fixture.files.len(),
        "DELTA live count",
    )?;
    next_fixture.global_matches = 9;
    let owners = client.stage_mutations(ingress, mutations, &fixture.budget())?;
    require_equal(&owners.count, &4, "DELTA mutation count")?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(old_view), &fixture.budget())?
            .source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation.producer,
        stream: operation.stream,
        stream_epoch: operation.stream_epoch,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    let request = UpdateRequest {
        operation,
        expected_view: old_view,
        kind: UpdateKind::SourceAdvance(transition),
    };
    let (view, elapsed) = publish_sources(client, daemon, request.clone(), &fixture.budget())?;
    eprintln!("native lexical DELTA published in {elapsed:?}");
    require_equal(
        &view.sequence,
        &old_view
            .sequence
            .checked_add(1)
            .ok_or("view sequence overflow")?,
        "one DELTA successor",
    )?;
    require_equal(
        &published(&client.submit(request, &fixture.budget())?)?,
        &view,
        "same operation replay view",
    )?;
    let latest = ViewSelection::Latest {
        workspace: operation.workspace,
        epoch: operation.epoch,
    };
    require_equal(
        &client.read_view(latest, &fixture.budget())?.identity,
        &view,
        "latest after replay",
    )?;
    let hashes = next_fixture
        .files
        .iter()
        .map(|file| (path(file), Sha256::digest(file.content.as_bytes()).into()))
        .collect();
    Ok(Successor {
        operation,
        view,
        fixture: next_fixture,
        hashes,
        owners: next_owners,
        refs: next_refs,
    })
}

fn check_foreign_cursor(
    client: &NativeUdsClient,
    successor: &Successor,
    old_cursor: NativeTextCursor,
) -> TestResult {
    let budget = successor.fixture.budget();
    let session =
        client.open_read_session(ViewSelection::Exact(successor.view), [0x46; 32], &budget)?;
    let checked = (|| -> TestResult {
        let query = global_page::query(successor.fixture.rank_page_size);
        let instance = text_spec().instance;
        // Send the original cursor over UDS, past the SDK binding guard.
        let result = client.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
                session: session.session,
                instance,
                query: query.clone(),
                after: Some(old_cursor),
            }),
            &budget,
        );
        require(
            matches!(&result, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput),
            format!("successor must refuse the old cursor: {result:?}"),
        )?;
        let fresh =
            client.search_text_page_in_read_session(&session, instance, query, None, &budget)?;
        require_equal(
            &fresh.matched,
            &9,
            "fresh DELTA matches after invalid cursor",
        )?;
        Ok(())
    })();
    let cleanup = client.release_read_session(&session, &budget);
    checked?;
    let (released, _) = cleanup?;
    let settlement = client.read_session_settlement(&released, &budget)?;
    require_equal(
        &settlement.view,
        &successor.view,
        "successor session settlement view",
    )
}
