//! Repository and language filters through the public SDK client under the
//! daemon's configured per-query work limits and interactive memory need.
//!
//! `corp-a:src/lib.rs` is Rust and `corp-b:src/lib.rs` is Python; both hold
//! `needle`. Every expected row follows from those two records and the
//! overlay rows each case adds.

use super::repository_identity::{configure_empty_view, repository_owner, stage_source, update};
use super::*;
use quanta_index_contract::{
    RepoId,
    lex::LanguageCode,
    native_protocol::{NativeOverlayTextQuery, NativeOverlayTextSource},
};
use quanta_index_sdk::native_uds::native_code_search_request;

const ALPHA: u8 = 20;
const BETA: u8 = 21;
const CREATED: u8 = 22;

fn publish_languages(client: &NativeUdsClient) -> TestResult<ViewRef> {
    let configured = configure_empty_view(client)?;
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let alpha = stage_source(
        client,
        ingress,
        &RepoId::try_from("corp-a")?,
        ("src/lib.rs", "needle alpha"),
        Some(LanguageCode::new("rust")?),
    )?;
    let beta = stage_source(
        client,
        ingress,
        &RepoId::try_from("corp-b")?,
        ("src/lib.rs", "needle beta"),
        Some(LanguageCode::new("python")?),
    )?;
    let owners = client.stage_mutations(
        ingress,
        [(ALPHA, alpha), (BETA, beta)]
            .into_iter()
            .map(|(id, record)| OwnerMutation {
                owner: repository_owner(id),
                expected: None,
                next: Some(record),
            }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(configured), &budget())?
            .source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    published(
        client,
        key,
        &client.submit(
            UpdateRequest {
                operation: key,
                expected_view: configured,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &budget(),
        )?,
    )
}

/// `request` narrowed to the any-of `languages` constraint.
fn with_languages(
    mut request: TextQueryRequest,
    languages: &[&str],
) -> TestResult<TextQueryRequest> {
    request.constraints = request.constraints.with_languages(
        languages
            .iter()
            .map(|language| LanguageCode::new(*language))
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(request)
}

/// The default `.text()` file page, which is a `select:path` projection, as
/// `(owner id, "repository:path")` ordered by owner.
fn code_search_rows(
    client: &NativeUdsClient,
    selected: ViewRef,
    languages: &[&str],
) -> TestResult<Vec<(u8, String)>> {
    let result = client
        .search_text_paths(
            ViewSelection::Exact(selected),
            native_text_spec().instance,
            with_languages(native_code_search_request("needle", 8), languages)?,
            &budget(),
        )
        .map_err(|error| format!("code search {languages:?}: {error:?}"))?;
    let mut rows = Vec::new();
    for (hit, repository) in result
        .ranked
        .ranking
        .hits
        .iter()
        .zip(&result.ranked.repositories)
    {
        let id = hit.owner.stable_id.0.first().copied().ok_or("owner id")?;
        rows.push((id, format!("{}:{}", repository.as_str(), hit.path)));
    }
    rows.sort_unstable();
    Ok(rows)
}

/// Durable rows as `(owner id, path)`, ordered by owner.
fn durable_rows(
    client: &NativeUdsClient,
    selected: ViewRef,
    request: TextQueryRequest,
) -> TestResult<Vec<(u8, String)>> {
    let label = format!("{:?}", request.query_text);
    let result = client
        .search_text(
            ViewSelection::Exact(selected),
            native_text_spec().instance,
            request,
            &budget(),
        )
        .map_err(|error| format!("durable query {label}: {error:?}"))?;
    let mut rows = Vec::new();
    for hit in result.hits {
        let id = hit.owner.stable_id.0.first().copied().ok_or("owner id")?;
        rows.push((id, hit.path));
    }
    rows.sort_unstable();
    Ok(rows)
}

/// Overlay rows as `(owner id, unsaved, "repository:path")`, ordered by owner.
fn overlay_rows(
    client: &NativeUdsClient,
    selected: ViewRef,
    session: NativeOverlaySession,
    request: TextQueryRequest,
) -> TestResult<Vec<(u8, bool, String)>> {
    let label = format!("{:?}", request.query_text);
    let result = client
        .search_overlay_text(
            NativeOverlayTextQuery {
                selection: ViewSelection::Exact(selected),
                instance: native_text_spec().instance,
                session,
                query: request,
                expected_corpus: None,
            },
            &budget(),
        )
        .map_err(|error| format!("overlay query {label}: {error:?}"))?;
    let mut rows = Vec::new();
    for hit in result.hits {
        let id = hit.owner.stable_id.0.first().copied().ok_or("owner id")?;
        let repository = hit.repository.as_ref().map_or("", RepoId::as_str);
        rows.push((
            id,
            matches!(hit.source, NativeOverlayTextSource::Overlay { .. }),
            format!("{repository}:{}", hit.path),
        ));
    }
    rows.sort_unstable();
    Ok(rows)
}

fn owned(rows: &[(u8, &str)]) -> Vec<(u8, String)> {
    rows.iter()
        .map(|(id, path)| (*id, (*path).to_owned()))
        .collect()
}

fn owned_overlay(rows: &[(u8, bool, &str)]) -> Vec<(u8, bool, String)> {
    rows.iter()
        .map(|(id, unsaved, identity)| (*id, *unsaved, (*identity).to_owned()))
        .collect()
}

/// The most `repo:` filters one query may carry, all regexes over `corp-a`.
const FOUR_REPOSITORY_REGEXES: &str = "needle repo:corp repo:^corp-a$ repo:a$ repo:c.rp-[a-z]";
/// A content regex beside the four `repo:` filters.
const REGEX_AND_FOUR_REPOSITORIES: &str =
    "/n.edle/ repo:corp repo:^corp-a$ repo:a$ repo:c.rp-[a-z]";

#[test]
fn repository_and_language_filters_fit_the_configured_query_budget() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let selected = publish_languages(&client)?;

    // Without an overlay.
    let alpha = owned(&[(ALPHA, "src/lib.rs")]);
    let beta = owned(&[(BETA, "src/lib.rs")]);
    for (languages, expected) in [
        (
            &[][..],
            owned(&[(ALPHA, "corp-a:src/lib.rs"), (BETA, "corp-b:src/lib.rs")]),
        ),
        (&["rust"][..], owned(&[(ALPHA, "corp-a:src/lib.rs")])),
        (&["python"][..], owned(&[(BETA, "corp-b:src/lib.rs")])),
    ] {
        let rows = code_search_rows(&client, selected, languages)?;
        require(
            rows == expected,
            "language constraint selected the wrong file",
        )?;
    }
    for (query, expected) in [
        ("needle repo:corp-a", &alpha),
        ("needle repo:corp-b", &beta),
        (FOUR_REPOSITORY_REGEXES, &alpha),
        (REGEX_AND_FOUR_REPOSITORIES, &alpha),
    ] {
        let rows = durable_rows(&client, selected, text_request(query))?;
        if &rows != expected {
            return Err(format!("{query}: {rows:?} != {expected:?}").into());
        }
    }

    // With an overlay: corp-a's file is edited and corp-b gains a new file.
    let live = LivePublisher::begin(&client, Duration::from_secs(30))?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher: live.id(),
        epoch: 1,
    };
    let document = |id| NativeOverlayDocument {
        session,
        owner: repository_owner(id),
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "overlay session did not begin",
    )?;
    update(
        &client,
        &live,
        document(ALPHA),
        None,
        1,
        Some("needle draft"),
        None,
        None,
    )?;
    // The durable-owner edit keeps its record's language.
    for (languages, expected) in [
        (
            &["rust"][..],
            owned_overlay(&[(ALPHA, true, "corp-a:src/lib.rs")]),
        ),
        (
            &["python"][..],
            owned_overlay(&[(BETA, false, "corp-b:src/lib.rs")]),
        ),
    ] {
        // Overlay reads have no path projection; the constraint is the same.
        let request = with_languages(text_request("needle"), languages)?;
        let rows = overlay_rows(&client, selected, session, request)?;
        require(
            rows == expected,
            "overlay language constraint selected the wrong row",
        )?;
    }
    update(
        &client,
        &live,
        document(CREATED),
        None,
        1,
        Some("needle fresh"),
        Some("src/new.rs"),
        Some(repository_owner(BETA)),
    )?;
    for (query, expected) in [
        (
            "needle repo:corp-a",
            owned_overlay(&[(ALPHA, true, "corp-a:src/lib.rs")]),
        ),
        (
            "needle repo:corp-b",
            owned_overlay(&[
                (BETA, false, "corp-b:src/lib.rs"),
                (CREATED, true, "corp-b:src/new.rs"),
            ]),
        ),
        (
            FOUR_REPOSITORY_REGEXES,
            owned_overlay(&[(ALPHA, true, "corp-a:src/lib.rs")]),
        ),
        (
            REGEX_AND_FOUR_REPOSITORIES,
            owned_overlay(&[(ALPHA, true, "corp-a:src/lib.rs")]),
        ),
    ] {
        let rows = overlay_rows(&client, selected, session, text_request(query))?;
        require(
            rows == expected,
            "overlay repo: filter selected the wrong row",
        )?;
    }
    live.renew()?;
    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "overlay session did not close",
    )?;
    running.stop()
}

/// The most content regexes one query may carry.
const FOUR_CONTENT_REGEXES: &str = "/needle/ /n.edle/ /alpha/ /^needle/";
/// Content regexes beside every other admitted filter family: three content
/// engines, two path engines, a source predicate and the metadata filters.
const CONTENT_REGEXES_AND_FILTERS: &str = "/needle/ /n.edle/ /alpha/ \
    file:src file:lib[.]rs file.contains('alpha') repo:corp-a lang:rust";
/// The most content regexes beside a path filter.
const FOUR_CONTENT_REGEXES_AND_PATH: &str = "/needle/ /n.edle/ /alpha/ /^needle/ file:src";
/// The most content regexes beside every other admitted filter family.
const FOUR_CONTENT_REGEXES_AND_FILTERS: &str = "/needle/ /n.edle/ /alpha/ /^needle/ \
    file:src file:lib[.]rs file.contains('alpha') repo:corp-a lang:rust";
/// The worst admissible query: the most content regexes, `file:` filters and
/// `repo:` filters, plus a source and a path predicate.
const WORST_ADMISSIBLE: &str = "/needle/ /n.edle/ /alpha/ /^needle/ \
    file:src file:lib file:[.]rs file:^src/ \
    repo:corp repo:^corp-a$ repo:a$ repo:c.rp-[a-z] \
    file.contains('alpha') path.contains('lib')";
/// Every query shape the content-regex budget test runs.
const CONTENT_REGEX_QUERIES: [&str; 5] = [
    FOUR_CONTENT_REGEXES,
    CONTENT_REGEXES_AND_FILTERS,
    FOUR_CONTENT_REGEXES_AND_PATH,
    FOUR_CONTENT_REGEXES_AND_FILTERS,
    WORST_ADMISSIBLE,
];

#[test]
fn content_regexes_fit_the_configured_query_budget() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let selected = publish_languages(&client)?;
    let alpha = owned(&[(ALPHA, "src/lib.rs")]);
    for query in CONTENT_REGEX_QUERIES {
        let rows = durable_rows(&client, selected, text_request(query))?;
        if rows != alpha {
            return Err(format!("{query}: {rows:?} != {alpha:?}").into());
        }
    }

    let live = LivePublisher::begin(&client, Duration::from_secs(30))?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher: live.id(),
        epoch: 1,
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "overlay session did not begin",
    )?;
    update(
        &client,
        &live,
        NativeOverlayDocument {
            session,
            owner: repository_owner(ALPHA),
        },
        None,
        1,
        Some("needle alpha draft"),
        None,
        None,
    )?;
    let edited = owned_overlay(&[(ALPHA, true, "corp-a:src/lib.rs")]);
    for query in CONTENT_REGEX_QUERIES {
        let rows = overlay_rows(&client, selected, session, text_request(query))?;
        if rows != edited {
            return Err(format!("overlay {query}: {rows:?} != {edited:?}").into());
        }
    }
    live.renew()?;
    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "overlay session did not close",
    )?;
    running.stop()
}
