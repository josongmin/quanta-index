//! Source-derived global BM25 oracle; no index/ranker/tokenizer is consulted.

use super::*;
use crate::scale_source_fixture::ScopedFile;
use quanta_index_contract::index::ViewRef;

const QUERY: &str = "scale_needle_token";
const PAGE: u32 = 256;

struct Gold {
    path: String,
    owner: OwnerKey,
    source: ObjectRef,
    score: f64,
}

fn gold(
    files: &[ScopedFile],
    expected_owners: &BTreeMap<String, OwnerKey>,
    source_refs: &BTreeMap<String, ObjectRef>,
) -> TestResult<Vec<Gold>> {
    require_equal(
        &files.len(),
        &LARGE_SOURCE_COUNT,
        "global oracle source count",
    )?;
    let mut statistics = Vec::with_capacity(files.len());
    let mut total_tokens = 0_u64;
    let mut document_frequency = 0_u32;
    for file in files {
        require(
            file.content.is_ascii(),
            "global oracle only covers the fixed ASCII generator",
        )?;
        let mut length = 0_u32;
        let mut frequency = 0_u32;
        for token in file
            .content
            .split(|byte: char| !byte.is_ascii_alphanumeric() && byte != '_')
            .filter(|token| !token.is_empty())
        {
            require(
                token.len() <= 256,
                "global oracle encountered a non-indexable generated run",
            )?;
            length = length
                .checked_add(1)
                .ok_or("oracle document length overflow")?;
            if token.eq_ignore_ascii_case(QUERY) {
                frequency = frequency
                    .checked_add(1)
                    .ok_or("oracle frequency overflow")?;
            }
        }
        require(frequency > 0, "global fixture lacks planted query")?;
        document_frequency = document_frequency
            .checked_add(1)
            .ok_or("oracle df overflow")?;
        total_tokens = total_tokens
            .checked_add(u64::from(length))
            .ok_or("oracle token sum overflow")?;
        statistics.push((file, length, frequency));
    }
    let n = f64::from(u32::try_from(files.len())?);
    let df = f64::from(document_frequency);
    let average = f64::from(u32::try_from(total_tokens)?) / n;
    let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
    let mut rows = Vec::with_capacity(files.len());
    for (file, length, frequency) in statistics {
        let path = format!("{}/{}", file.source_repo_id, file.repo_relative_path);
        let tf = f64::from(frequency);
        // Standard BM25 equation, independent of the product's scaled arithmetic.
        let score = idf * (tf * 2.2) / (tf + 1.2 * (0.25 + 0.75 * f64::from(length) / average));
        require(
            score.is_finite() && score > 0.0,
            "global gold score is invalid",
        )?;
        rows.push(Gold {
            owner: *expected_owners
                .get(&path)
                .ok_or("global gold owner absent")?,
            source: *source_refs.get(&path).ok_or("global gold source absent")?,
            path,
            score,
        });
    }
    rows.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.owner.cmp(&right.owner))
    });
    Ok(rows)
}

pub(super) fn observe(
    client: &NativeUdsClient,
    view: ViewRef,
    files: &[ScopedFile],
    expected_owners: &BTreeMap<String, OwnerKey>,
    source_refs: &BTreeMap<String, ObjectRef>,
    open_key: [u8; 32],
) -> TestResult<Vec<(String, OwnerKey, ObjectRef, u64)>> {
    let gold = gold(files, expected_owners, source_refs)?;
    let mut session =
        client.open_read_session(ViewSelection::Exact(view), open_key, &large_budget())?;
    let result = (|| -> TestResult<Vec<(String, OwnerKey, ObjectRef, u64)>> {
        let mut observed = Vec::with_capacity(gold.len());
        let mut after = None;
        for page_index in 0..=LARGE_SOURCE_COUNT / usize::try_from(PAGE)? {
            let page = client.search_text_page_in_read_session(
                &session,
                text_spec().instance,
                TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: QUERY.into(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    top_k: PAGE,
                    cursor: None,
                },
                after,
                &large_budget(),
            )?;
            require_equal(&page.view, &view, "global page selected view")?;
            require_equal(
                &page.matched,
                &u64::try_from(gold.len())?,
                "global eligible count",
            )?;
            require_equal(
                &page.remaining,
                &u64::try_from(gold.len() - observed.len())?,
                "global remaining count",
            )?;
            require(
                !page.hits.is_empty(),
                format!("global page{page_index} is empty"),
            )?;
            for hit in page.hits {
                let expected = gold
                    .get(observed.len())
                    .ok_or("global page returned an extra source")?;
                require_equal(&hit.path, &expected.path, "independent global rank path")?;
                require_equal(&hit.owner, &expected.owner, "independent global rank owner")?;
                require_equal(
                    &hit.source,
                    &expected.source,
                    "independent global source ref",
                )?;
                require(
                    (hit.score.get() - expected.score).abs()
                        <= 1e-12 * expected.score.abs().max(1.0),
                    format!(
                        "global BM25 score differs: path={} observed={} gold={}",
                        hit.path,
                        hit.score.get(),
                        expected.score
                    ),
                )?;
                observed.push((hit.path, hit.owner, hit.source, hit.score.get().to_bits()));
            }
            after = page.after;
            if after.is_none() {
                break;
            }
            // A new page is a new request; lease renewal is explicit and never
            // refreshes a running query's work budget or deadline.
            session = client.renew_read_session(&session, &large_budget())?;
        }
        require_equal(
            &observed.len(),
            &LARGE_SOURCE_COUNT,
            "all global ranks observed exactly once",
        )?;
        require(
            after.is_none(),
            "global final page still has a continuation",
        )?;
        Ok(observed)
    })();
    // Cleanup runs even after oracle/query failure. Preserve the primary cause.
    let cleanup = (|| -> TestResult {
        let (released, _) = client.release_read_session(&session, &large_budget())?;
        let settlement = client.read_session_settlement(&released, &large_budget())?;
        require_equal(&settlement.view, &view, "global session settlement view")?;
        Ok(())
    })();
    match (result, cleanup) {
        (Err(primary), _) => Err(primary),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Ok(rows), Ok(())) => Ok(rows),
    }
}
