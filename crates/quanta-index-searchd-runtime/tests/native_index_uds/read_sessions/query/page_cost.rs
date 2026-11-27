//! 09.4a/09.4b: the work of reading one fixed view's retained text pages to
//! the end, without (before) and with (after) the session's rank-order cache.
//!
//! The fixture is `count` one-line Sources `alpha`, Tree layout, so the query
//! `alpha` matches every Source. Without the cache, every page re-plans and
//! re-scans the whole candidate union to report an exact `matched`. With `S(N)` the scan cost of an `N`-document union and `h` the
//! per-row projection cost, one page costs `S(N) + h * rows`, so reading to
//! the end costs `(N / k) * S(N) + h * N` with `S(N)` linear in `N`: the
//! total is quadratic in `N` for a fixed page size `k`. The assertions are the
//! consequences of that model that hold for any `S` and `h`, derived by hand
//! from the fixture; exact per-page constants depend on the Tree map layout
//! and are reported, not asserted.
//!
//! With the cache, the first page scans once and keeps the full rank order;
//! a later page binary-searches its cursor and projects its own rows, so it
//! costs `O(k)` reads, independent of `N`, and reading to the end costs
//! `S(N) + h * N`. Every cached page must be byte-identical to its re-scanned
//! counterpart. The before/after table is printed with the test output.

use super::*;
use quanta_index_searchd_runtime::native::NativeRetainedReadWork;
use std::sync::Mutex;

/// Publish `count` Sources `doc-<i>.rs` with body `alpha`, owners 1..=count.
fn publish_uniform<P: AsRef<std::path::Path>>(
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
        while ordinal < count && inputs.len() < 100 {
            ordinal = ordinal.checked_add(1).ok_or("ordinal overflow")?;
            inputs.push(SourceInput {
                path: format!("doc-{ordinal:04}.rs"),
                bytes: b"alpha\n".to_vec(),
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
        let staged = client.stage_sources(ingress, inputs, &budget())?;
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

/// Per-page `(hits, matched, query reads, query bytes)` of one full sequence.
type PageCost = (usize, u64, u64, u64);

/// One full sequence: per-page costs and canonical page bytes.
type Pages = (Vec<PageCost>, Vec<Vec<u8>>);

fn read_to_end(
    client: &NativeUdsClient<std::path::PathBuf>,
    view: quanta_index_contract::index::ViewRef,
    page_size: u32,
    observed: &Mutex<Vec<NativeRetainedReadWork>>,
    open_key: u8,
) -> TestResult<Pages> {
    let mut session =
        client.open_read_session(ViewSelection::Exact(view), [open_key; 32], &budget())?;
    let instance = native_text_spec().instance;
    let mut after = None;
    let mut costs = Vec::new();
    let mut bytes = Vec::new();
    let mut owners = std::collections::BTreeSet::new();
    loop {
        // The caller owns its lease; renewal is control work, not a query.
        session = client
            .renew_read_session(&session, &budget())
            .map_err(|error| format!("renew after {} pages: {error:?}", costs.len()))?;
        let before = observed
            .lock()
            .map_err(|_poison| "observer poisoned")?
            .len();
        let page = client
            .search_text_page_in_read_session(
                &session,
                instance,
                native_text_request("alpha", page_size),
                after,
                &budget(),
            )
            .map_err(|error| format!("page {} of size {page_size}: {error:?}", costs.len()))?;
        let reported = observed
            .lock()
            .map_err(|_poison| "observer poisoned")?
            .get(before..)
            .map(<[NativeRetainedReadWork]>::to_vec)
            .ok_or("observation lost")?;
        let [work] = reported.as_slice() else {
            return Err("one page must report one observation".into());
        };
        let work = *work;
        assert!(work.succeeded);
        assert_eq!(work.session, session.session);
        for hit in &page.hits {
            assert!(owners.insert(hit.owner), "page repeated an owner");
        }
        costs.push((
            page.hits.len(),
            page.matched,
            work.query.reads,
            work.query.bytes,
        ));
        after = page.after;
        let mut canonical = page;
        canonical.session = quanta_index_contract::native_protocol::NativeReadSessionId([0; 32]);
        bytes.push(serde_json::to_vec(&canonical)?);
        if after.is_none() {
            break;
        }
    }
    let _released = client.release_read_session(&session, &budget())?;
    Ok((costs, bytes))
}

/// One sequence: `(documents, page size, cached, per-page costs)`.
type Sequence = (u64, u32, bool, Vec<PageCost>);

fn total_reads(costs: &[PageCost]) -> TestResult<u64> {
    costs.iter().try_fold(0_u64, |total, (_, _, reads, _)| {
        total
            .checked_add(*reads)
            .ok_or_else(|| "read total overflow".into())
    })
}

fn sequence(
    report: &[Sequence],
    count: u64,
    page_size: u32,
    cached: bool,
) -> TestResult<&[PageCost]> {
    report
        .iter()
        .find(|(documents, size, mode, _)| {
            *documents == count && *size == page_size && *mode == cached
        })
        .map(|(_, _, _, costs)| costs.as_slice())
        .ok_or_else(|| format!("sequence {count}/{page_size}/{cached} missing").into())
}

/// Start a service on `root` with the cache on or off and read both page
/// sizes to the end over the published view.
fn read_sizes(
    root: &std::path::Path,
    socket: &std::path::Path,
    view: Option<quanta_index_contract::index::ViewRef>,
    count: u64,
    cached: bool,
) -> TestResult<(quanta_index_contract::index::ViewRef, Vec<(u32, Pages)>)> {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let service = read_session_service_with_config(
        root,
        socket,
        Duration::from_secs(60),
        if cached {
            NativeReadSessionConfig::default()
        } else {
            NativeReadSessionConfig {
                text_order_memory: 0,
                ..NativeReadSessionConfig::default()
            }
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
        let client = NativeUdsClient::new(socket.to_path_buf());
        let view = match view {
            Some(view) => view,
            None => publish_uniform(&client, count)
                .map_err(|error| format!("publish {count}: {error}"))?,
        };
        let mut sizes = Vec::new();
        for (page_size, key) in [(10_u32, 71_u8), (100, 72)] {
            sizes.push((
                page_size,
                read_to_end(&client, view, page_size, &observed, key)?,
            ));
        }
        Ok((view, sizes))
    })();
    let stopped = running.stop();
    let read = primary?;
    stopped?;
    Ok(read)
}

/// The largest read count of any page after the first.
fn later_reads(costs: &[PageCost]) -> TestResult<u64> {
    costs
        .iter()
        .skip(1)
        .map(|cost| cost.2)
        .max()
        .ok_or_else(|| "a sequence needs a later page".into())
}

#[test]
#[ignore = "heavy: 1,000 Sources paged to the end 220 times; run with `just rust-test-native-page-cost`"]
fn retained_text_pages_rescan_without_the_order_cache_and_reuse_it_with_it() -> TestResult {
    let mut report: Vec<Sequence> = Vec::new();
    for count in [100_u64, 1_000] {
        let root = private_root()?;
        let socket = root.path().join("native.sock");
        let (view, scanned) = read_sizes(root.path(), &socket, None, count, false)?;
        let (_view, cached) = read_sizes(root.path(), &socket, Some(view), count, true)?;
        for (
            (page_size, (scanned_costs, scanned_bytes)),
            (cached_size, (cached_costs, cached_bytes)),
        ) in scanned.into_iter().zip(cached)
        {
            assert_eq!(page_size, cached_size);
            // Byte-identical pages, cursors and exact counts.
            assert_eq!(cached_bytes, scanned_bytes, "{count}/{page_size}");
            report.push((count, page_size, false, scanned_costs));
            report.push((count, page_size, true, cached_costs));
        }
    }
    let mut table = String::from(
        "N | k | mode | pages | first-page reads | max later-page reads | total reads\n",
    );
    for (count, page_size, cached, costs) in &report {
        use std::fmt::Write as _;
        writeln!(
            table,
            "{count} | {page_size} | {} | {} | {} | {} | {}",
            if *cached { "cached" } else { "re-scan" },
            costs.len(),
            costs.first().map_or(0, |cost| cost.2),
            later_reads(costs)?,
            total_reads(costs)?,
        )?;
    }
    // The before/after table is this heavy recipe's report.
    #[expect(
        clippy::print_stdout,
        reason = "the heavy recipe reports its cost table"
    )]
    {
        println!("{table}");
    }
    let context = format!("{table}{report:?}");
    for (count, page_size, _cached, costs) in &report {
        // Every page reports the exact count of the whole union, and the
        // sequence has exactly N / k full pages.
        assert_eq!(
            u64::try_from(costs.len())?,
            count / u64::from(*page_size),
            "{context}"
        );
        for (rows, matched, _, _) in costs {
            assert_eq!(*matched, *count, "{context}");
            assert_eq!(u32::try_from(*rows)?, *page_size, "{context}");
        }
    }
    let mut scans = Vec::new();
    for count in [100_u64, 1_000] {
        let mut per_size = Vec::new();
        for page_size in [10_u32, 100] {
            let cached = sequence(&report, count, page_size, true)?;
            let scanned = sequence(&report, count, page_size, false)?;
            // The first page is the re-scan's first page; capturing the
            // order reads nothing more.
            assert_eq!(
                cached.first().map(|cost| cost.2),
                scanned.first().map(|cost| cost.2),
                "{count}/{page_size}: {context}"
            );
            // A re-scanned page costs the union scan `S` plus its rows'
            // projection; a cached page only the same projection. So the
            // difference is one constant on every later page, whatever the
            // rows' map-node alignment: later pages cost O(k), not O(N).
            let differences: Vec<i128> = scanned
                .iter()
                .zip(cached)
                .skip(1)
                .map(|(scanned, cached)| i128::from(scanned.2) - i128::from(cached.2))
                .collect();
            let scan = *differences.first().ok_or("no later page")?;
            assert!(scan > 0, "{count}/{page_size}: {context}");
            assert!(
                differences.iter().all(|difference| *difference == scan),
                "{count}/{page_size}: {context}"
            );
            // Reading to the end costs one scan plus every row's projection.
            let pages = i128::try_from(cached.len())?;
            assert_eq!(
                i128::from(total_reads(cached)?),
                i128::from(total_reads(scanned)?) - (pages - 1) * scan,
                "{count}/{page_size}: {context}"
            );
            per_size.push(scan);
        }
        // The skipped scan is the same union scan for either page size.
        assert!(
            per_size.windows(2).all(|pair| pair.first() == pair.last()),
            "{context}"
        );
        scans.push(*per_size.first().ok_or("no page size")?);
    }
    // The skipped scan grows with the corpus; cached pages do not pay it.
    assert!(
        scans.is_sorted_by(|smaller, larger| smaller < larger),
        "{context}"
    );
    Ok(())
}
