//! 09.4b: retained text pages reuse the session's cached rank order.
//!
//! The first page of a `(view, instance, query)` computes the full ranked
//! order once and caches it in the session; later pages binary-search their
//! cursor in that order and project only their own rows. The oracle is the
//! same daemon with the cache disabled, which re-scans on every page: every
//! page must be byte-identical. Work is read from the per-request telemetry
//! and checked against the algorithm, with its constants counted from the
//! sequences themselves rather than guessed:
//!
//! - a re-scanned page costs the whole-union scan `S` plus the projection of
//!   its own rows; a cached owner page costs only that same projection. So
//!   `re-scanned - cached` is one constant `D` (the scan) on every later
//!   page, whatever rows or map-node alignment the page has, and `D` grows
//!   with `N` while the cached page does not pay it;
//! - a cached path page also binary-searches its cursor: at most
//!   `ceil(log2(groups + 1))` probes, each one record read, no more than one
//!   row's projection, so its difference varies by at most that much;
//! - the first page is the re-scan's first page plus the capture, which reads
//!   nothing: reading to the end costs one scan plus the row projections.
//!
//! Path pages compare their per-row `CodeSearch` witnesses and excerpts too:
//! a cached row restores the witness captured with the order.

use super::*;
use quanta_index_contract::RepoId;
use quanta_index_contract::native_protocol::{
    NativeCodeSearchMatch, NativeReadSessionId, NativeTextPage, NativeTextPathPageV1,
};
use quanta_index_sdk::native_uds::native_code_search_request;
use quanta_index_searchd_runtime::native::NativeRetainedReadWork;
use std::sync::Mutex;

const REPOSITORY: &str = "repo-pages";
const PAGE: u32 = 4;

/// Body `i`: `alpha` one to three times and `beta` zero to four times, so
/// BM25 and the Legacy `CodeSearch` score both vary and tie.
fn body(ordinal: u64) -> String {
    let alpha = ["alpha ", "alpha alpha ", "alpha alpha alpha "];
    let beta = [
        "",
        "beta ",
        "beta beta ",
        "beta beta beta ",
        "beta beta beta beta ",
    ];
    let pick = |table: &[&'static str], modulus: u64| -> &'static str {
        ordinal
            .checked_rem(modulus)
            .and_then(|index| table.get(usize::try_from(index).map_or(usize::MAX, |index| index)))
            .copied()
            .map_or("", |piece| piece)
    };
    format!("{}{}\n", pick(&alpha, 3), pick(&beta, 5))
}

/// Publish `count` repository Sources `doc-<i>.rs`.
fn publish<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    count: u64,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let configured = create_text_workspace(client)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let mut mutations = Vec::new();
    let mut ordinal = 0_u64;
    while ordinal < count {
        let mut inputs = Vec::new();
        let mut owners = Vec::new();
        while ordinal < count && inputs.len() < 64 {
            ordinal = ordinal.checked_add(1).ok_or("ordinal overflow")?;
            inputs.push(SourceInput {
                path: format!("doc-{ordinal:04}.rs"),
                bytes: body(ordinal).into_bytes(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: None,
            });
            let mut stable = [0_u8; 16];
            let (prefix, _) = stable.split_at_mut(8);
            prefix.copy_from_slice(&ordinal.to_be_bytes());
            owners.push(OwnerKey {
                producer: ingress.operation.producer,
                stable_id: StableOwnerId(stable),
            });
        }
        let NativeIndexResponse::StagedSources(staged) = client.request(
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository: RepoId::new(REPOSITORY)?,
                inputs,
            },
            &budget(),
        )?
        else {
            return Err("sources were not staged".into());
        };
        if staged.len() != owners.len() {
            return Err("one input must stage one Source".into());
        }
        for (owner, source) in owners.into_iter().zip(staged) {
            mutations.push(OwnerMutation {
                owner,
                expected: None,
                next: Some(source),
            });
        }
    }
    let request = prepare_text_mutations_in_ingress(client, configured, ingress, mutations)?;
    // A loaded host may outlast one protocol step; settle the outcome.
    crate::publication_settle::submit_and_settle(client, &request, || {
        RequestBudgetV1::for_duration(Duration::from_secs(600))
    })
}

/// One page's canonical bytes with the session handle normalized, and the
/// query reads it charged.
type Paged = (Vec<u8>, u64);

fn query_reads(
    observed: &Mutex<Vec<NativeRetainedReadWork>>,
    before: usize,
    session: NativeReadSessionId,
) -> TestResult<u64> {
    let reported = observed
        .lock()
        .map_err(|_poison| "observer poisoned")?
        .get(before..)
        .map(<[NativeRetainedReadWork]>::to_vec)
        .ok_or("observation lost")?;
    let [work] = reported.as_slice() else {
        return Err("one page must report one observation".into());
    };
    assert!(work.succeeded);
    assert_eq!(work.session, session);
    Ok(work.query.reads)
}

fn query_bytes(observed: &Mutex<Vec<NativeRetainedReadWork>>, before: usize) -> TestResult<u64> {
    Ok(observed
        .lock()
        .map_err(|_poison| "observer poisoned")?
        .get(before)
        .ok_or("observation lost")?
        .query
        .bytes)
}

fn observed_len(observed: &Mutex<Vec<NativeRetainedReadWork>>) -> TestResult<usize> {
    Ok(observed
        .lock()
        .map_err(|_poison| "observer poisoned")?
        .len())
}

/// Read every owner-ranked page of `alpha`, then every `CodeSearch` file page.
fn read_all(
    client: &NativeUdsClient<std::path::PathBuf>,
    view: quanta_index_contract::index::ViewRef,
    observed: &Mutex<Vec<NativeRetainedReadWork>>,
) -> TestResult<(Vec<Paged>, Vec<Paged>)> {
    let mut session = client.open_read_session(ViewSelection::Exact(view), [81; 32], &budget())?;
    let instance = native_text_spec().instance;
    let mut owner_pages = Vec::new();
    let mut after = None;
    loop {
        // The caller owns its lease on a loaded host; renewal is not a query.
        session = client.renew_read_session(&session, &budget())?;
        let before = observed_len(observed)?;
        let mut page: NativeTextPage = client.search_text_page_in_read_session(
            &session,
            instance,
            native_text_request("alpha", PAGE),
            after,
            &budget(),
        )?;
        let reads = query_reads(observed, before, session.session)?;
        after = page.after;
        page.session = NativeReadSessionId([0; 32]);
        owner_pages.push((serde_json::to_vec(&page)?, reads));
        if after.is_none() {
            break;
        }
    }
    let mut path_pages = Vec::new();
    let mut after = None;
    loop {
        session = client.renew_read_session(&session, &budget())?;
        let before = observed_len(observed)?;
        let mut page: NativeTextPathPageV1 = client.search_text_path_page_in_read_session(
            &session,
            instance,
            native_code_search_request("alpha", PAGE),
            after.clone(),
            &budget(),
        )?;
        let reads = query_reads(observed, before, session.session)?;
        page.validate_code_search()?;
        // Every row's body holds `alpha` on its single line, so each row
        // carries a five-byte line-1 content witness with its excerpt: the
        // byte comparison covers the witness and the excerpt of every row.
        assert_eq!(page.code_search.len(), page.ranked.ranking.hits.len());
        for matched in &page.code_search {
            let Some(NativeCodeSearchMatch::Content {
                start,
                end,
                start_line: 1,
                end_line: 1,
                excerpt: Some(_),
            }) = matched
            else {
                return Err(format!("row witness differs: {matched:?}").into());
            };
            assert_eq!(end.checked_sub(*start), Some(5));
        }
        after.clone_from(&page.after);
        page.session = NativeReadSessionId([0; 32]);
        path_pages.push((serde_json::to_vec(&page)?, reads));
        if after.is_none() {
            break;
        }
    }
    let _released = client.release_read_session(&session, &budget())?;
    Ok((owner_pages, path_pages))
}

/// Publish `count` Sources and read all pages with `text_order_memory`
/// declared for cached orders (`0`: no cache).
fn run(count: u64, text_order_memory: u64) -> TestResult<(Vec<Paged>, Vec<Paged>)> {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let observed = Arc::new(Mutex::new(Vec::new()));
    let service = read_session_service_with_config(
        root.path(),
        &socket,
        Duration::from_secs(60),
        NativeReadSessionConfig {
            text_order_memory,
            ..NativeReadSessionConfig::default()
        },
    )?;
    let sink = Arc::clone(&observed);
    service.observe_retained_read_work(Arc::new(move |work| {
        if let Ok(mut observed) = sink.lock() {
            observed.push(work);
        }
    }))?;
    let running = NativeServiceGuard::start(service);
    let primary = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket.clone());
        let view = publish(&client, count)?;
        read_all(&client, view, &observed)
    })();
    let stopped = running.stop();
    let pages = primary?;
    stopped?;
    Ok(pages)
}

/// Two distinct queries share one session and a share large enough for one
/// order, but not both. Each query's second page must use its own capture.
fn run_replacement(text_order_memory: u64) -> TestResult<([Vec<Paged>; 2], Paged, [u64; 2])> {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let observed = Arc::new(Mutex::new(Vec::new()));
    let service = read_session_service_with_config(
        root.path(),
        &socket,
        Duration::from_secs(60),
        NativeReadSessionConfig {
            text_order_memory,
            ..NativeReadSessionConfig::default()
        },
    )?;
    let sink = Arc::clone(&observed);
    service.observe_retained_read_work(Arc::new(move |work| {
        if let Ok(mut observed) = sink.lock() {
            observed.push(work);
        }
    }))?;
    let running = NativeServiceGuard::start(service);
    let primary = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket.clone());
        let view = publish(&client, 1_280)?;
        let mut session =
            client.open_read_session(ViewSelection::Exact(view), [82; 32], &budget())?;
        let instance = native_text_spec().instance;
        assert_ne!(
            quanta_index_contract::native_protocol::native_text_query_digest(
                instance,
                &native_text_request("alpha", PAGE)
            )
            .map_err(std::io::Error::other)?,
            quanta_index_contract::native_protocol::native_text_query_digest(
                instance,
                &native_text_request("beta", PAGE)
            )
            .map_err(std::io::Error::other)?
        );
        let mut pages = [Vec::new(), Vec::new()];
        let mut first_page_bytes = [0; 2];
        let mut alpha_after = None;
        let mut mismatched_alpha = None;
        for (term_index, (term, term_pages)) in
            ["alpha", "beta"].into_iter().zip(&mut pages).enumerate()
        {
            let mut after = None;
            for page_index in 0..2 {
                session = client.renew_read_session(&session, &budget())?;
                let before = observed_len(&observed)?;
                let mut page: NativeTextPage = client.search_text_page_in_read_session(
                    &session,
                    instance,
                    native_text_request(term, PAGE),
                    after,
                    &budget(),
                )?;
                let reads = query_reads(&observed, before, session.session)?;
                if page_index == 0 {
                    first_page_bytes[term_index] = query_bytes(&observed, before)?;
                }
                after = page.after;
                page.session = NativeReadSessionId([0; 32]);
                term_pages.push((serde_json::to_vec(&page)?, reads));
                assert!(after.is_some(), "{term} must have a second page");
                if term_index == 0 && page_index == 1 {
                    alpha_after = after;
                }
                if term_index == 1 && page_index == 0 {
                    // A mismatched continuation cannot evict the new B order.
                    session = client.renew_read_session(&session, &budget())?;
                    let before = observed_len(&observed)?;
                    let mut old: NativeTextPage = client.search_text_page_in_read_session(
                        &session,
                        instance,
                        native_text_request("alpha", PAGE),
                        alpha_after,
                        &budget(),
                    )?;
                    let reads = query_reads(&observed, before, session.session)?;
                    old.session = NativeReadSessionId([0; 32]);
                    mismatched_alpha = Some((serde_json::to_vec(&old)?, reads));
                }
            }
        }
        let _released = client.release_read_session(&session, &budget())?;
        Ok((pages, mismatched_alpha.ok_or("mismatched continuation absent")?, first_page_bytes))
    })();
    let stopped = running.stop();
    let pages = primary?;
    stopped?;
    Ok(pages)
}

fn bytes(pages: &[Paged]) -> Vec<&[u8]> {
    pages.iter().map(|(page, _)| page.as_slice()).collect()
}

fn ceil_log2(value: u64) -> u64 {
    u64::from(u64::BITS.saturating_sub(value.saturating_sub(1).leading_zeros()))
}

/// `re-scanned - cached` reads of every later page.
fn differences(scanned: &[Paged], cached: &[Paged]) -> Vec<i128> {
    scanned
        .iter()
        .zip(cached)
        .skip(1)
        .map(|((_, scanned), (_, cached))| i128::from(*scanned).saturating_sub(i128::from(*cached)))
        .collect()
}

#[test]
fn cached_pages_equal_rescanned_pages_and_cost_only_their_rows() -> TestResult {
    let mut scans = Vec::new();
    for count in [22_u64, 86] {
        let (cached_owner, cached_path) =
            run(count, NativeReadSessionConfig::default().text_order_memory)?;
        let (scanned_owner, scanned_path) = run(count, 0)?;
        let reads = |pages: &[Paged]| pages.iter().map(|(_, reads)| *reads).collect::<Vec<_>>();
        let context = format!(
            "N={count} cached owner {:?} scanned owner {:?} cached path {:?} scanned path {:?}",
            reads(&cached_owner),
            reads(&scanned_owner),
            reads(&cached_path),
            reads(&scanned_path),
        );
        #[expect(
            clippy::print_stdout,
            reason = "the measured reads are this test's report"
        )]
        {
            println!("{context}");
        }
        // Byte-identical pages, cursors and exact counts, page by page.
        assert_eq!(bytes(&cached_owner), bytes(&scanned_owner), "{context}");
        assert_eq!(bytes(&cached_path), bytes(&scanned_path), "{context}");
        let pages = usize::try_from(count.div_ceil(u64::from(PAGE)))?;
        assert_eq!(cached_owner.len(), pages, "{context}");
        assert_eq!(cached_path.len(), pages, "{context}");
        // The first page is the same scan; capturing reads nothing more.
        assert_eq!(
            cached_owner.first().map(|page| page.1),
            scanned_owner.first().map(|page| page.1),
            "{context}"
        );
        assert_eq!(
            cached_path.first().map(|page| page.1),
            scanned_path.first().map(|page| page.1),
            "{context}"
        );
        // Every later cached owner page skips exactly the scan `D`.
        let owner = differences(&scanned_owner, &cached_owner);
        let scan = *owner.first().ok_or("no later page")?;
        assert!(scan > 0, "{context}");
        assert!(
            owner.iter().all(|difference| *difference == scan),
            "{context}"
        );
        scans.push(scan);
        // A cached path page skips its scan, less at most its cursor search:
        // `ceil(log2(groups + 1))` probes, each no dearer than one projected
        // row, which costs at most a later owner page divided by its rows.
        let path = differences(&scanned_path, &cached_path);
        assert!(path.iter().all(|difference| *difference > 0), "{context}");
        let row = i128::from(
            cached_owner
                .iter()
                .skip(1)
                .map(|(_, reads)| *reads)
                .max()
                .ok_or("no later page")?
                .div_ceil(u64::from(PAGE)),
        );
        let spread = path
            .iter()
            .max()
            .ok_or("no later page")?
            .saturating_sub(*path.iter().min().ok_or("no later page")?);
        assert!(
            spread <= i128::from(ceil_log2(count.saturating_add(1))).saturating_mul(row),
            "{context}"
        );
    }
    // The skipped scan grows with the corpus; the cached page does not pay it.
    assert!(
        scans.is_sorted_by(|smaller, larger| smaller < larger),
        "{scans:?}"
    );
    Ok(())
}

#[test]
fn first_page_replaces_a_different_cached_order_with_one_order_share() -> TestResult {
    let share = u64::try_from(std::mem::size_of::<quanta_index_lexical::native::TextRankEntry>())?
        .checked_mul(1_280)
        .ok_or("rank order share overflow")?;
    let ([cached_alpha, cached_beta], cached_mismatch, cached_first_bytes) = run_replacement(share)?;
    let ([scanned_alpha, scanned_beta], scanned_mismatch, scanned_first_bytes) = run_replacement(0)?;
    assert_eq!(cached_first_bytes, scanned_first_bytes);
    assert_eq!(bytes(&cached_alpha), bytes(&scanned_alpha));
    assert_eq!(bytes(&cached_beta), bytes(&scanned_beta));
    assert_eq!(cached_alpha[0].1, scanned_alpha[0].1);
    assert!(cached_alpha[1].1 < scanned_alpha[1].1);
    assert_eq!(cached_beta[0].1, scanned_beta[0].1);
    assert!(cached_beta[1].1 < scanned_beta[1].1);
    assert_eq!(cached_mismatch, scanned_mismatch);
    Ok(())
}

/// An order its declared share cannot fund is dropped, never a refusal.
///
/// A share of one byte funds no order, so every page re-ranks exactly as
/// with no cache, page for page in bytes and in reads.
#[test]
fn an_unfunded_order_re_ranks_every_page_with_identical_results() -> TestResult {
    let (unfunded_owner, unfunded_path) = run(22, 1)?;
    let (scanned_owner, scanned_path) = run(22, 0)?;
    let reads = |pages: &[Paged]| pages.iter().map(|(_, reads)| *reads).collect::<Vec<_>>();
    assert_eq!(bytes(&unfunded_owner), bytes(&scanned_owner));
    assert_eq!(bytes(&unfunded_path), bytes(&scanned_path));
    assert_eq!(reads(&unfunded_owner), reads(&scanned_owner));
    assert_eq!(reads(&unfunded_path), reads(&scanned_path));
    Ok(())
}

/// A share the Interactive plan cannot hold refuses the configuration.
///
/// The order share is part of the registry's one Interactive grant, so the
/// refusal comes before any session exists; the same registry without the
/// share is admitted.
#[test]
fn an_order_share_beyond_the_interactive_plan_is_refused_at_configuration() -> TestResult {
    let root = private_root()?;
    let sessions = NativeReadSessionConfig {
        global_limit: 256,
        ..NativeReadSessionConfig::default()
    };
    let share = NativeReadSessionConfig::full_text_order_memory(sessions.global_limit)?;
    // The read-session test plan gives the Interactive class 64 MiB.
    assert!(share > 67_108_864, "{share}");
    let refused = read_session_service_with_config(
        root.path(),
        &root.path().join("refused.sock"),
        Duration::from_secs(60),
        NativeReadSessionConfig {
            text_order_memory: share,
            ..sessions
        },
    );
    assert!(
        matches!(
            refused
                .as_ref()
                .map_err(|error| error.downcast_ref::<IndexError>()),
            Err(Some(IndexError::ResourceLimited))
        ),
        "{:?}",
        refused.err()
    );
    let admitted = read_session_service_with_config(
        root.path(),
        &root.path().join("admitted.sock"),
        Duration::from_secs(60),
        NativeReadSessionConfig {
            text_order_memory: 0,
            ..sessions
        },
    )?;
    drop(admitted);
    Ok(())
}
