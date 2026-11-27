//! Selected-source preview through the SDK and pinned Legacy daemon.
#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::time::Duration;

use quanta_index_contract::{
    GenerationPin, HighlightSpan, ManifestGeneration, PreviewByteRange, PreviewKind,
    PreviewUnavailableReason, RepoId, RevisionId,
};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    legacy_reference::{LegacySearchdBinary, SearchdBinaryProcess, publish_legacy_source},
    private_tempdir,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

fn publish_preview_corpus(
    binary: &LegacySearchdBinary,
    state: &Path,
    files: &[(&str, &str, u64)],
) -> Result<(SearchdBinaryProcess, GenerationPin), Box<dyn Error>> {
    let daemon = binary.start(state, 8)?;
    let client = daemon.connect()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let (_head, _receipt) = publish_legacy_source(&client, &repo, &revision, 1, None, files, None)?;
    Ok((
        daemon,
        GenerationPin::new(repo, revision, ManifestGeneration::new(1)),
    ))
}

fn wait_for_preview_queries(client: &QuantaIndex) -> TestResult {
    let _ready = wait_for(
        &RealTicker::new(),
        Duration::from_secs(20),
        Duration::from_millis(50),
        "pinned old-Q preview query backend ready",
        || client.observability().process_readiness(),
        |report| report.ready,
        |_| true,
    )?;
    Ok(())
}

fn assert_preview(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query: &str,
    path: &str,
    raw: &str,
    original_focus: PreviewByteRange,
    normalized_focus: PreviewByteRange,
    normalization_equivalent: bool,
) -> TestResult {
    let response = client
        .lexical()
        .query()
        .native(query)
        .pinned(pin.clone())
        .top_k(10)
        .execute()?;
    if response.generation != *pin || response.results.len() != 1 {
        return Err(format!("{query:?}: unexpected SDK page: {response:?}").into());
    }
    let candidate = response.results.first().ok_or("selected hit missing")?;
    if candidate.repo_relative_path.as_str() != path {
        return Err(format!("{query:?}: selected the wrong source: {candidate:?}").into());
    }
    candidate
        .validate_source_metadata()
        .map_err(str::to_owned)?;
    let preview = candidate
        .preview
        .as_ref()
        .ok_or("preview metadata missing")?;
    if preview.kind != PreviewKind::SourceChunk
        || preview.unavailable_reason.is_some()
        || preview.chunk_start_byte != Some(0)
        || preview.original_focus != Some(original_focus)
        || preview.normalized_focus != Some(normalized_focus)
        || preview.normalization_equivalent != normalization_equivalent
        || preview.source != candidate.source
    {
        return Err(format!("{query:?}: incorrect source-bound preview: {candidate:?}").into());
    }
    let context = preview.original_context.ok_or("source context missing")?;
    let context_start = usize::try_from(context.start)?;
    let context_end = usize::try_from(context.end)?;
    if raw.get(context_start..context_end)
        != Some(
            candidate
                .excerpt
                .as_ref()
                .ok_or("fixture row carries no match excerpt")?
                .snippet
                .as_str(),
        )
    {
        return Err(format!("{query:?}: emitted snippet differs from immutable source").into());
    }
    let expected_focus = raw
        .get(usize::try_from(original_focus.start)?..usize::try_from(original_focus.end)?)
        .ok_or("independent focus oracle is not UTF-8 aligned")?;
    let highlighted = candidate
        .excerpt
        .as_ref()
        .ok_or("fixture row carries no match excerpt")?
        .highlights
        .iter()
        .any(|span| {
            let (Ok(start), Ok(len)) = (usize::try_from(span.start), usize::try_from(span.len))
            else {
                return false;
            };
            candidate
                .excerpt
                .as_ref()
                .and_then(|excerpt| excerpt.snippet.get(start..start.saturating_add(len)))
                == Some(expected_focus)
        });
    if !highlighted {
        return Err(format!("{query:?}: expected focus absent from emitted highlights").into());
    }
    Ok(())
}

fn assert_path_preview(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    for query in ["select:path needlepath", "needlepath"] {
        let response = client
            .lexical()
            .query()
            .native(query)
            .pinned(pin.clone())
            .top_k(10)
            .execute()?;
        let candidate = response.results.first().ok_or("path hit missing")?;
        if response.generation != *pin
            || response.results.len() != 1
            || candidate.repo_relative_path.as_str() != "src/needlepath.rs"
            || candidate
                .excerpt
                .as_ref()
                .ok_or("fixture row carries no match excerpt")?
                .snippet
                != "src/needlepath.rs"
            || !candidate
                .excerpt
                .as_ref()
                .ok_or("fixture row carries no match excerpt")?
                .highlights
                .is_empty()
            || candidate
                .excerpt
                .as_ref()
                .ok_or("fixture row carries no match excerpt")?
                .snippet_hit_offset
                .is_some()
        {
            return Err(format!("{query:?}: incorrect path-only candidate: {response:?}").into());
        }
        candidate
            .validate_source_metadata()
            .map_err(str::to_owned)?;
        let preview = candidate.preview.as_ref().ok_or("path preview missing")?;
        if preview.kind != PreviewKind::Path
            || preview.unavailable_reason.is_some()
            || preview.original_focus.is_some()
            || preview.original_context.is_some()
            || preview.normalized_focus.is_some()
        {
            return Err(format!(
                "{query:?}: path match fabricated content coordinates: {candidate:?}"
            )
            .into());
        }
    }
    // A path-only match must not make the negative Boolean content clause true.
    assert_preview(
        client,
        pin,
        "unrelated AND NOT needlepath",
        "src/needlepath.rs",
        "unrelated content",
        PreviewByteRange { start: 0, end: 9 },
        PreviewByteRange { start: 0, end: 9 },
        false,
    )
}

fn assert_unavailable(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query: &str,
    path: &str,
    reason: PreviewUnavailableReason,
) -> TestResult {
    let response = client
        .lexical()
        .query()
        .native(query)
        .pinned(pin.clone())
        .top_k(1)
        .execute()?;
    let candidate = response.results.first().ok_or("expected hit missing")?;
    if response.generation != *pin
        || response.results.len() != 1
        || candidate.repo_relative_path.as_str() != path
        || !candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .snippet
            .is_empty()
        || !candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .highlights
            .is_empty()
        || candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .snippet_hit_offset
            .is_some()
        || !candidate.preview.as_ref().is_some_and(|preview| {
            preview.kind == PreviewKind::SourceChunk
                && preview.unavailable_reason == Some(reason)
                && preview.original_focus.is_none()
                && preview.original_context.is_none()
                && preview.normalized_focus.is_none()
        })
    {
        return Err(
            format!("{query:?}: optional refusal changed selected hit: {response:?}").into(),
        );
    }
    candidate
        .validate_source_metadata()
        .map_err(str::to_owned)?;
    Ok(())
}

fn assert_multihit_highlights(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let response = client
        .lexical()
        .query()
        .native("threehits")
        .pinned(pin.clone())
        .top_k(1)
        .execute()?;
    let candidate = response.results.first().ok_or("multi-hit source missing")?;
    let expected = [
        HighlightSpan { start: 0, len: 9 },
        HighlightSpan { start: 10, len: 9 },
        HighlightSpan { start: 20, len: 9 },
    ];
    if response.results.len() != 1
        || candidate.repo_relative_path.as_str() != "src/multi.rs"
        || candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .snippet
            != "threehits threehits threehits"
        || candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .highlights
            .as_slice()
            != expected.as_slice()
        || candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .snippet_hit_offset
            != Some(0)
    {
        return Err(format!("multi-hit SDK spans are incomplete: {response:?}").into());
    }
    candidate
        .validate_source_metadata()
        .map_err(str::to_owned)?;
    Ok(())
}

fn assert_overlapping_raw_highlights(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let response = client
        .lexical()
        .query()
        .native("'aba'")
        .pinned(pin.clone())
        .top_k(1)
        .execute()?;
    let candidate = response.results.first().ok_or("overlap source missing")?;
    let expected = [
        HighlightSpan { start: 0, len: 3 },
        HighlightSpan { start: 2, len: 3 },
    ];
    if response.results.len() != 1
        || candidate.repo_relative_path.as_str() != "src/overlap.rs"
        || candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .snippet
            != "ababa"
        || candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .highlights
            .as_slice()
            != expected.as_slice()
        || candidate
            .excerpt
            .as_ref()
            .ok_or("fixture row carries no match excerpt")?
            .snippet_hit_offset
            != Some(0)
    {
        return Err(format!("overlapping raw SDK spans are incomplete: {response:?}").into());
    }
    candidate
        .validate_source_metadata()
        .map_err(str::to_owned)?;
    Ok(())
}

#[test]
fn l4_sdk_preview_uses_matcher_ranges_and_original_source_bytes() -> TestResult {
    let parent = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let prefix = "context ".repeat(80);
    let upper = format!("{prefix}NEEDLECASE");
    let regex = format!("{prefix}needle42");
    let decomposed = format!("{prefix}cafe\u{301}");
    let long_focus = format!("{prefix}{}", "A".repeat(200));
    let oversized_focus = format!("{prefix}{}", "B".repeat(241));
    let files = [
        ("src/upper.rs", upper.as_str(), 1),
        ("src/regex.rs", regex.as_str(), 2),
        ("src/decomposed.rs", decomposed.as_str(), 3),
        ("src/long200.rs", long_focus.as_str(), 4),
        ("src/long241.rs", oversized_focus.as_str(), 5),
        ("src/needlepath.rs", "unrelated content", 6),
        ("src/multi.rs", "threehits threehits threehits", 7),
        ("src/overlap.rs", "ababa", 8),
    ];
    let (daemon, pin) = publish_preview_corpus(&binary, &parent.path().join("state"), &files)?;
    let client = daemon.connect()?;
    wait_for_preview_queries(&client)?;
    assert_preview(
        &client,
        &pin,
        "needlecase",
        "src/upper.rs",
        &upper,
        PreviewByteRange {
            start: 640,
            end: 650,
        },
        PreviewByteRange {
            start: 640,
            end: 650,
        },
        false,
    )?;
    assert_preview(
        &client,
        &pin,
        "/needle[0-9]+/",
        "src/regex.rs",
        &regex,
        PreviewByteRange {
            start: 640,
            end: 648,
        },
        PreviewByteRange {
            start: 640,
            end: 648,
        },
        false,
    )?;
    for native in [
        "/[a-z]+[0-9]+/ case:yes",
        "/[a-z]+[0-9]+/ case:yes index:no",
    ] {
        assert_preview(
            &client,
            &pin,
            native,
            "src/regex.rs",
            &regex,
            PreviewByteRange {
                start: 640,
                end: 648,
            },
            PreviewByteRange {
                start: 640,
                end: 648,
            },
            false,
        )?;
    }
    assert_preview(
        &client,
        &pin,
        "café",
        "src/decomposed.rs",
        &decomposed,
        PreviewByteRange {
            start: 640,
            end: 646,
        },
        PreviewByteRange {
            start: 640,
            end: 645,
        },
        true,
    )?;
    assert_preview(
        &client,
        &pin,
        "/A{200}/",
        "src/long200.rs",
        &long_focus,
        PreviewByteRange {
            start: 640,
            end: 840,
        },
        PreviewByteRange {
            start: 640,
            end: 840,
        },
        false,
    )?;
    assert_unavailable(
        &client,
        &pin,
        "/B{241}/",
        "src/long241.rs",
        PreviewUnavailableReason::FocusExceedsBudget,
    )?;
    assert_path_preview(&client, &pin)?;
    assert_multihit_highlights(&client, &pin)?;
    assert_overlapping_raw_highlights(&client, &pin)?;
    drop(client);
    daemon.stop()?;
    binary.verify()
}

#[test]
fn l4_sdk_preview_survives_daemon_process_restart() -> TestResult {
    let parent = private_tempdir()?;
    let state = parent.path().join("state");
    let checkout = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let checkout_file = checkout.path().join("checkout.rs");
    std::fs::write(&checkout_file, "oldanchor")?;
    let overflow = "a".repeat(35);
    let checkout_body = std::fs::read_to_string(&checkout_file)?;
    let files = [
        ("src/decomposed.rs", "cafe\u{301}", 1),
        ("src/regex.rs", "needle42", 2),
        ("src/multi.rs", "threehits threehits threehits", 3),
        ("src/overlap.rs", "ababa", 4),
        ("src/overflow.rs", overflow.as_str(), 5),
        ("src/checkout.rs", checkout_body.as_str(), 6),
    ];
    let (producer, pin) = publish_preview_corpus(&binary, &state, &files)?;
    producer.stop()?;
    std::fs::write(&checkout_file, "newanchor")?;

    for round in 0..2 {
        if round == 1 {
            std::fs::remove_file(&checkout_file)?;
        }
        let daemon = binary
            .start(&state, 8)
            .map_err(|error| format!("daemon start {round} failed: {error}"))?;
        let client = daemon.connect()?;
        wait_for_preview_queries(&client)?;
        assert_preview(
            &client,
            &pin,
            "café",
            "src/decomposed.rs",
            "cafe\u{301}",
            PreviewByteRange { start: 0, end: 6 },
            PreviewByteRange { start: 0, end: 5 },
            true,
        )?;
        assert_multihit_highlights(&client, &pin)?;
        for native in [
            "/[a-z]+[0-9]+/ case:yes",
            "/[a-z]+[0-9]+/ case:yes index:no",
        ] {
            assert_preview(
                &client,
                &pin,
                native,
                "src/regex.rs",
                "needle42",
                PreviewByteRange { start: 0, end: 8 },
                PreviewByteRange { start: 0, end: 8 },
                false,
            )?;
        }
        assert_overlapping_raw_highlights(&client, &pin)?;
        assert_unavailable(
            &client,
            &pin,
            "'aaa'",
            "src/overflow.rs",
            PreviewUnavailableReason::WorkBudget,
        )?;
        assert_preview(
            &client,
            &pin,
            "oldanchor",
            "src/checkout.rs",
            "oldanchor",
            PreviewByteRange { start: 0, end: 9 },
            PreviewByteRange { start: 0, end: 9 },
            false,
        )?;
        let changed = client
            .lexical()
            .query()
            .native("newanchor")
            .pinned(pin.clone())
            .top_k(1)
            .execute()?;
        if changed.generation != pin || !changed.results.is_empty() {
            return Err(format!("mutable checkout leaked into pinned result: {changed:?}").into());
        }
        drop(client);
        daemon.stop()?;
    }
    binary.verify()
}
