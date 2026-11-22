//! Exact Native regex results through the ordinary SDK.

use super::*;

fn assert_text_query_owners(
    sources: &[(u64, u8, &str, &str)],
    probes: &[(&str, &[u8])],
    passes: usize,
) -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let mut view = create_text_workspace(&client)?;
        for &(sequence, id, path, body) in sources {
            (view, _) = publish_text_owner(&client, view, sequence, id, path, body)?;
        }

        let instance = native_text_spec().instance;
        let top_k = u32::try_from(sources.len())?;
        for pass in 0..passes {
            for &(query, expected) in probes {
                let result = client.search_text(
                    ViewSelection::Exact(view),
                    instance,
                    native_text_request(query, top_k),
                    &budget(),
                )?;
                assert_eq!(result.view, view, "pass {pass}: {query}");
                assert_eq!(result.instance, instance, "pass {pass}: {query}");
                assert!(result.exhaustive, "pass {pass}: {query}");
                assert_eq!(
                    result.matched,
                    u64::try_from(expected.len())?,
                    "pass {pass}: {query}"
                );

                let mut owners: Vec<_> =
                    result.hits.iter().map(|hit| hit.owner.stable_id).collect();
                owners.sort_unstable();
                assert_eq!(
                    owners,
                    expected
                        .iter()
                        .map(|id| StableOwnerId([*id; 16]))
                        .collect::<Vec<_>>(),
                    "pass {pass}: {query}"
                );
            }
        }
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[test]
fn alternation_regex_returns_documents_matching_any_branch() -> TestResult {
    assert_text_query_owners(
        &[
            (2, 1, "src/left.rs", "alphamarker only here"),
            (3, 2, "src/right.rs", "betamarker only here"),
            (4, 3, "src/both.rs", "alphamarker and betamarker"),
            (5, 4, "src/none.rs", "neither token present"),
        ],
        &[
            ("/(alphamarker|betamarker)/", &[1, 2, 3]),
            ("/alphamarker/", &[1, 3]),
            ("/zzzmissing/", &[]),
        ],
        1,
    )
}

#[test]
fn case_folded_literal_regex_agrees_with_the_keyword_route() -> TestResult {
    assert_text_query_owners(
        &[
            (2, 1, "src/a.rs", "gamma_replacement freshsentinel"),
            (3, 2, "src/b.rs", "unrelated content without the token"),
        ],
        &[
            ("freshsentinel case:no", &[1]),
            ("/freshsentinel/ case:no", &[1]),
            ("/fresh/ case:no", &[1]),
            ("/sentinel/ case:no", &[1]),
            ("/gamma/ case:no", &[1]),
        ],
        1,
    )
}

#[test]
fn internal_conjunction_short_branch_and_unicode_keep_truth_on_repeated_queries() -> TestResult {
    assert_text_query_owners(
        &[
            (2, 1, "src/both.rs", "foo then bar"),
            (3, 2, "src/prefix.rs", "foo alone"),
            (4, 3, "src/short.rs", "x"),
            (5, 4, "src/unicode.rs", "freſh then bar"),
        ],
        &[
            ("/foo.*bar/ case:no", &[1]),
            ("/x|foo.*bar/ case:no", &[1, 3]),
            ("/fresh.*bar/ case:no", &[4]),
            ("/(?:foo)?x/ case:no", &[3]),
        ],
        2,
    )
}
