//! Selected-source preview through the SDK and query socket.
#![forbid(unsafe_code)]

#[path = "common/searchd_binary_process.rs"]
mod searchd_binary_process;

use std::error::Error;

use quanta_index_contract::{
    GenerationPin, HighlightSpan, PreviewByteRange, PreviewKind, PreviewUnavailableReason,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use quanta_index_searchd_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

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
        .top_k(1)
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
    if raw.get(context_start..context_end) != Some(candidate.snippet.as_str()) {
        return Err(format!("{query:?}: emitted snippet differs from immutable source").into());
    }
    let expected_focus = raw
        .get(usize::try_from(original_focus.start)?..usize::try_from(original_focus.end)?)
        .ok_or("independent focus oracle is not UTF-8 aligned")?;
    let highlighted = candidate.highlights.iter().any(|span| {
        let (Ok(start), Ok(len)) = (usize::try_from(span.start), usize::try_from(span.len)) else {
            return false;
        };
        candidate.snippet.get(start..start.saturating_add(len)) == Some(expected_focus)
    });
    if !highlighted {
        return Err(format!("{query:?}: expected focus absent from emitted highlights").into());
    }
    Ok(())
}

fn assert_path_preview(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let response = client
        .lexical()
        .query()
        .native("select:path needlepath")
        .pinned(pin.clone())
        .top_k(1)
        .execute()?;
    let candidate = response.results.first().ok_or("path hit missing")?;
    if response.results.len() != 1
        || candidate.repo_relative_path.as_str() != "src/needlepath.rs"
        || candidate.snippet != "src/needlepath.rs"
        || !candidate.highlights.is_empty()
        || candidate.snippet_hit_offset.is_some()
    {
        return Err(format!("incorrect path-only candidate: {response:?}").into());
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
        return Err(format!("path match fabricated content coordinates: {candidate:?}").into());
    }
    Ok(())
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
    if response.results.len() != 1
        || candidate.repo_relative_path.as_str() != path
        || !candidate.snippet.is_empty()
        || !candidate.highlights.is_empty()
        || candidate.snippet_hit_offset.is_some()
        || candidate
            .preview
            .as_ref()
            .and_then(|preview| preview.unavailable_reason)
            != Some(reason)
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
        || candidate.snippet != "threehits threehits threehits"
        || candidate.highlights.as_slice() != expected.as_slice()
        || candidate.snippet_hit_offset != Some(0)
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
        || candidate.snippet != "ababa"
        || candidate.highlights.as_slice() != expected.as_slice()
        || candidate.snippet_hit_offset != Some(0)
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
    let mut runtime = E2eRuntime::boot()?;
    let prefix = "context ".repeat(80);
    let upper = format!("{prefix}NEEDLECASE");
    let regex = format!("{prefix}needle42");
    let decomposed = format!("{prefix}cafe\u{301}");
    let long_focus = format!("{prefix}{}", "A".repeat(200));
    let oversized_focus = format!("{prefix}{}", "B".repeat(241));
    runtime.ingest_text("repo", "src/upper.rs", &upper)?;
    runtime.ingest_text("repo", "src/regex.rs", &regex)?;
    runtime.ingest_text("repo", "src/decomposed.rs", &decomposed)?;
    runtime.ingest_text("repo", "src/long200.rs", &long_focus)?;
    runtime.ingest_text("repo", "src/long241.rs", &oversized_focus)?;
    runtime.ingest_text("repo", "src/needlepath.rs", "unrelated content")?;
    runtime.ingest_text("repo", "src/multi.rs", "threehits threehits threehits")?;
    runtime.ingest_text("repo", "src/overlap.rs", "ababa")?;
    let pin = runtime.generation_pin();
    let _sealed = runtime.seal()?;
    runtime.activate_last_sealed_generation()?;
    runtime.start()?;
    let (query_socket, control_socket, ingest_socket) = runtime
        .socket_paths()
        .ok_or("started harness has no query/control/ingest sockets")?;
    let client = QuantaIndex::connect(
        ConnectOptions::from_state_root(runtime.state_root())
            .with_query_socket(query_socket)
            .with_control_socket(control_socket)
            .with_ingest_socket(ingest_socket),
    )?;
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
    Ok(runtime.stop()?)
}

#[test]
fn l4_sdk_preview_survives_daemon_process_restart() -> TestResult {
    let state = quanta_index_searchd_harness::private_tempdir()?;
    let mut runtime = E2eRuntime::boot_in(state.path())?;
    runtime.ingest_text("repo", "src/decomposed.rs", "cafe\u{301}")?;
    runtime.ingest_text("repo", "src/multi.rs", "threehits threehits threehits")?;
    runtime.ingest_text("repo", "src/overlap.rs", "ababa")?;
    let pin = runtime.generation_pin();
    let _sealed = runtime.seal()?;
    runtime.activate_last_sealed_generation()?;
    // Activation uses the harness driver for its control CAS. Release the
    // runtime and its state-root lease before a separate process boots.
    runtime.stop()?;

    for round in 0..2 {
        let daemon = searchd_binary_process::SearchdBinaryProcess::start(state.path())
            .map_err(|error| format!("daemon start {round} failed: {error}"))?;
        let client = daemon.connect()?;
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
        assert_overlapping_raw_highlights(&client, &pin)?;
        drop(client);
        daemon.stop()?;
    }
    Ok(())
}
