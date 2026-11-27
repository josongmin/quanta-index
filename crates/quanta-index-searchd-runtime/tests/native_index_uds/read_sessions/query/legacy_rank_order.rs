//! SDK `.text()` file order on Native follows the Legacy `CodeSearch` score.
//!
//! The bodies are the fixed-score fixture of the Legacy
//! `code_search_fixed_scores_and_chunk_overlap_invariance` test. Every
//! expected score is computed by hand from the Legacy scorer: 100 for an
//! exact identifier boundary, 2 per extra occurrence (capped at three extra),
//! 5 for the exact spelling of a folded term, and `32 - gap` when all terms
//! lie within 32 bytes. A bare term matches the body or the path; a path
//! match adds 35 in the basename and 15 elsewhere, and the better of the
//! body and path witnesses counts. Only the `gamma` files have a query term
//! in their path. Equal scores keep the page order: repository, then path.

use super::protected_revision::{ProtectedRevision, anchored_service, publish_and_seal};
use super::*;
use quanta_index_contract::{RepoId, RevisionId};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};

const REVISION: ProtectedRevision<'static> = ProtectedRevision {
    repo: "repo-rank",
    revision: "rev-rank",
    slot: 45,
    owner_tag: 0x72,
};

fn files() -> Vec<(String, Vec<u8>)> {
    let distant = format!("alpha {} beta", "x".repeat(40));
    [
        ("src/single.rs", "alpha"),
        ("src/many.rs", "alpha alpha alpha alpha alpha"),
        ("src/adjacent.rs", "alpha beta"),
        ("src/distant.rs", distant.as_str()),
        ("src/plain.rs", "alpha beta alpha"),
        ("src/case_upper.rs", "Alpha"),
        ("src/case_lower.rs", "alpha"),
        ("src/gamma.rs", "nothing here"),
        ("src/gamma/mod.rs", "x = 1"),
        ("src/other.rs", "let gamma;"),
    ]
    .into_iter()
    .map(|(path, body)| (path.to_owned(), body.as_bytes().to_vec()))
    .collect()
}

fn ranked(client: &QuantaIndex, query: &str) -> TestResult<Vec<(String, f32)>> {
    let response = client
        .lexical()
        .query()
        .text(query)
        .active(
            RepoId::new(REVISION.repo)?,
            RevisionId::new(REVISION.revision)?,
        )
        .top_k(20)
        .execute()
        .map_err(|error| format!("{query}: {error:?}"))?;
    Ok(response
        .results
        .iter()
        .map(|row| (row.repo_relative_path.as_str().to_owned(), row.score))
        .collect())
}

fn expected(rows: &[(&str, f32)]) -> Vec<(String, f32)> {
    rows.iter()
        .map(|(path, score)| ((*path).to_owned(), *score))
        .collect()
}

#[test]
fn sdk_text_orders_files_by_the_legacy_code_search_score() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let _sources = publish_and_seal(root.path(), &socket, &REVISION, &files())?;
    let running = NativeServiceGuard::start(anchored_service(root.path(), &socket)?);
    let primary = (|| -> TestResult {
        let client = QuantaIndex::connect(
            ConnectOptions::from_native_socket(
                socket.clone(),
                ViewSelection::Latest {
                    workspace: REVISION.source_scope(),
                    epoch: 1,
                },
            )
            .with_native_text_instance(native_text_spec().instance),
        )?;
        // Sensitive `alpha`: many 100 + 3 extra * 2; plain 100 + 2.
        let alpha_order = expected(&[
            ("src/many.rs", 106.0),
            ("src/plain.rs", 102.0),
            ("src/adjacent.rs", 100.0),
            ("src/case_lower.rs", 100.0),
            ("src/distant.rs", 100.0),
            ("src/single.rs", 100.0),
        ]);
        assert_eq!(ranked(&client, "alpha case:yes")?, alpha_order);

        // The same hand-derived order must survive every public SDK page
        // boundary, including score ties ordered by Source path.
        let first = client
            .lexical()
            .query()
            .text("alpha case:yes")
            .active(
                RepoId::new(REVISION.repo)?,
                RevisionId::new(REVISION.revision)?,
            )
            .top_k(1)
            .execute()?;
        let pin = first.generation.clone();
        let mut page = first;
        for (index, row) in alpha_order.iter().enumerate() {
            assert_eq!(page.generation, pin, "page {index} changed generation");
            let [hit] = page.results.as_slice() else {
                return Err(format!("page {index} omitted or added a row").into());
            };
            assert_eq!(
                (hit.repo_relative_path.as_str(), hit.score),
                (row.0.as_str(), row.1),
                "page {index} changed rank order"
            );
            let remaining = alpha_order.len() - index;
            assert_eq!(
                page.window.candidate_count(),
                quanta_index_contract::CandidateCountV1::Exact(u64::try_from(remaining)?),
                "page {index} changed remaining count"
            );
            let has_next = remaining > 1;
            assert_eq!(page.window.has_more(), Some(has_next));
            assert_eq!(page.next_cursor.is_some(), has_next);
            if let Some(cursor) = page.next_cursor.take() {
                page = client
                    .lexical()
                    .query()
                    .text("alpha case:yes")
                    .pinned(pin.clone())
                    .top_k(1)
                    .after(cursor)
                    .execute()?;
            }
        }
        // Two terms: adjacent 200 + (32 - 1); plain 200 + 2 + (32 - 1);
        // distant has a 41-byte gap, beyond the proximity window.
        assert_eq!(
            ranked(&client, "alpha beta case:yes")?,
            expected(&[
                ("src/plain.rs", 233.0),
                ("src/adjacent.rs", 231.0),
                ("src/distant.rs", 200.0),
            ])
        );
        // Folded `Alpha`: only case_upper has the exact spelling (+5).
        assert_eq!(
            ranked(&client, "Alpha")?,
            expected(&[
                ("src/many.rs", 106.0),
                ("src/case_upper.rs", 105.0),
                ("src/plain.rs", 102.0),
                ("src/adjacent.rs", 100.0),
                ("src/case_lower.rs", 100.0),
                ("src/distant.rs", 100.0),
                ("src/single.rs", 100.0),
            ])
        );
        // Path-only matches: basename 100 + 35, directory 100 + 15; the body
        // match scores 100.
        assert_eq!(
            ranked(&client, "gamma case:yes")?,
            expected(&[
                ("src/gamma.rs", 135.0),
                ("src/gamma/mod.rs", 115.0),
                ("src/other.rs", 100.0),
            ])
        );
        // Folded: every witness has the query's exact spelling (+5).
        assert_eq!(
            ranked(&client, "gamma")?,
            expected(&[
                ("src/gamma.rs", 140.0),
                ("src/gamma/mod.rs", 120.0),
                ("src/other.rs", 105.0),
            ])
        );
        // `path:` scope never matches the body.
        assert_eq!(
            ranked(&client, "path:gamma case:yes")?,
            expected(&[("src/gamma.rs", 135.0), ("src/gamma/mod.rs", 115.0)])
        );
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

/// The Legacy frontdoor revision: its four regex queries each return one file.
const FRONTDOOR: ProtectedRevision<'static> = ProtectedRevision {
    repo: "repo-regex",
    revision: "rev-regex",
    slot: 46,
    owner_tag: 0x73,
};

fn frontdoor_files() -> Vec<(String, Vec<u8>)> {
    [
        ("src/lib.rs", "todo!()\nfn main() {}\n"),
        ("src/alpha.rs", "sphinx of quartz\n"),
        ("src/beta.rs", "sphinx riddles\n"),
        ("docs/readme.md", "quartz notes\n"),
    ]
    .into_iter()
    .map(|(path, body)| (path.to_owned(), body.as_bytes().to_vec()))
    .collect()
}

fn frontdoor_ranked(client: &QuantaIndex, query: &str) -> TestResult<Vec<(String, f32)>> {
    let response = client
        .lexical()
        .query()
        .text(query)
        .active(
            RepoId::new(FRONTDOOR.repo)?,
            RevisionId::new(FRONTDOOR.revision)?,
        )
        .top_k(20)
        .execute()
        .map_err(|error| format!("{query}: {error:?}"))?;
    Ok(response
        .results
        .iter()
        .map(|row| (row.repo_relative_path.as_str().to_owned(), row.score))
        .collect())
}

/// Ranked `(path, score)` rows of one Native `select:path` page.
fn path_page(
    client: &NativeUdsClient<&std::path::Path>,
    session: &quanta_index_contract::native_protocol::NativeReadSessionStatus,
    query: &str,
) -> TestResult<Vec<(String, f64)>> {
    let page = client.search_text_path_page_in_read_session(
        session,
        native_text_spec().instance,
        native_text_request(query, 20),
        None,
        &budget(),
    )?;
    Ok(page
        .ranked
        .ranking
        .hits
        .iter()
        .map(|hit| (hit.path.clone(), hit.score.get()))
        .collect())
}

/// Regex terms take the Legacy regex witness: the best boundary span of all
/// matches on the NFC surface, with no exact-case or proximity component.
#[test]
fn regex_terms_order_files_by_the_legacy_code_search_score() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let _sources = publish_and_seal(root.path(), &socket, &FRONTDOOR, &frontdoor_files())?;
    let running = NativeServiceGuard::start(anchored_service(root.path(), &socket)?);
    let primary = (|| -> TestResult {
        let client = QuantaIndex::connect(
            ConnectOptions::from_native_socket(
                socket.clone(),
                ViewSelection::Latest {
                    workspace: FRONTDOOR.source_scope(),
                    epoch: 1,
                },
            )
            .with_native_text_instance(native_text_spec().instance),
        )?;
        // The Legacy frontdoor queries. Each witness is an exact boundary
        // span of the body (100). In `sphinx regex:/quartz/` the folded
        // literal also has its exact spelling (+5); the regex term earns
        // neither exact case nor proximity.
        assert_eq!(
            frontdoor_ranked(&client, "regex:/sphinx.*quartz/")?,
            expected(&[("src/alpha.rs", 100.0)])
        );
        assert_eq!(
            frontdoor_ranked(&client, "content:regex:/sphinx.*riddles/")?,
            expected(&[("src/beta.rs", 100.0)])
        );
        assert_eq!(
            frontdoor_ranked(&client, "sphinx regex:/quartz/")?,
            expected(&[("src/alpha.rs", 205.0)])
        );

        // The Native leaves a CodeSearch regex lowers to, on a read session.
        let uds = NativeUdsClient::new(socket.as_path());
        let view = uds
            .read_view(
                ViewSelection::Latest {
                    workspace: FRONTDOOR.source_scope(),
                    epoch: 1,
                },
                &budget(),
            )?
            .identity;
        let session = uds.open_read_session(ViewSelection::Exact(view), [71; 32], &budget())?;
        // `path:regex:/beta[.]rs/`: a basename path witness, 100 + 35.
        assert_eq!(
            path_page(&uds, &session, "select:path path.regex('beta[.]rs')")?,
            [("src/beta.rs".to_owned(), 135.0)]
        );
        // Unscoped regex: body OR path. `alpha` matches only a basename.
        assert_eq!(
            path_page(
                &uds,
                &session,
                "select:path (/alpha/ OR path.regex('alpha'))"
            )?,
            [("src/alpha.rs".to_owned(), 135.0)]
        );
        // Body witnesses only: equal scores keep the repository/path order.
        assert_eq!(
            path_page(
                &uds,
                &session,
                "select:path (/quartz/ OR path.regex('quartz'))"
            )?,
            [
                ("docs/readme.md".to_owned(), 100.0),
                ("src/alpha.rs".to_owned(), 100.0),
            ]
        );
        let _released = uds.release_read_session(&session, &budget())?;
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}
