//! The serving boundary of a pinned track generation (QI-BB-003).
//!
//! One policy resolves every pin on both tracks: retained by the durable
//! authority → serveable; past the head or the head still being built →
//! `NOT_READY`; anything else → `UNKNOWN_GENERATION`. The table below is
//! the oracle: each row is one `(retained, materialized head, sealed head,
//! pin)` state and the one outcome the policy owes it.

#![forbid(unsafe_code)]

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, PinnedGenerationReadinessV1, UNKNOWN_GENERATION_CODE, validate_pinned_generation_v1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn g(value: u64) -> ManifestGeneration {
    ManifestGeneration::new(value)
}

fn pin(generation: u64) -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
        g(generation),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    Served,
    NotReady,
    Unknown,
}

fn outcome(result: Result<(), CoreError>) -> Result<Expected, Box<dyn std::error::Error>> {
    match result {
        Ok(()) => Ok(Expected::Served),
        Err(CoreError::NotReady(_)) => Ok(Expected::NotReady),
        Err(CoreError::Typed { code, .. }) if code == UNKNOWN_GENERATION_CODE => {
            Ok(Expected::Unknown)
        }
        Err(other) => Err(format!("unexpected refusal shape: {other:?}").into()),
    }
}

/// One row of the oracle: the readiness state a pin meets and the one
/// outcome it is owed.
struct Row {
    retained: bool,
    materialized: Option<u64>,
    sealed: Option<u64>,
    pin: u64,
    expected: Expected,
}

const fn row(
    retained: bool,
    materialized: Option<u64>,
    sealed: Option<u64>,
    pin: u64,
    expected: Expected,
) -> Row {
    Row {
        retained,
        materialized,
        sealed,
        pin,
        expected,
    }
}

#[test]
fn every_pin_state_has_exactly_one_outcome() -> TestResult {
    // (retained, materialized head, sealed head, pinned generation) -> outcome
    let table = [
        // Retained by the authority: served regardless of the heads.
        row(true, Some(5), Some(5), 5, Expected::Served),
        row(true, Some(5), Some(5), 2, Expected::Served),
        row(true, None, None, 2, Expected::Served),
        // Nothing materialized yet: not ready.
        row(false, None, None, 1, Expected::NotReady),
        // Past the materialized head: not ready (it may still arrive).
        row(false, Some(5), Some(5), 6, Expected::NotReady),
        // The head itself while it is still being built: not ready.
        row(false, Some(6), Some(5), 6, Expected::NotReady),
        row(false, Some(1), None, 1, Expected::NotReady),
        // The head, sealed in the track ledger but not retained by the
        // authority (a crash between seal and record): unknown, never
        // served from the sealed directory alone.
        row(false, Some(5), Some(5), 5, Expected::Unknown),
        // Older than the head and not retained: reaped or never sealed.
        row(false, Some(5), Some(5), 2, Expected::Unknown),
        row(false, Some(5), Some(5), 4, Expected::Unknown),
        row(false, Some(6), Some(5), 3, Expected::Unknown),
        row(false, Some(6), None, 3, Expected::Unknown),
    ];
    for case in table {
        for track in [
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Semantic,
        ] {
            let readiness = PinnedGenerationReadinessV1 {
                retained_by_authority: case.retained,
                materialized_head: case.materialized.map(g),
                sealed_head: case.sealed.map(g),
            };
            let observed = outcome(validate_pinned_generation_v1(
                "test",
                &pin(case.pin),
                track,
                readiness,
            ))?;
            if observed != case.expected {
                return Err(format!(
                    "{track:?}: retained={} materialized={:?} sealed={:?} pin={}: expected {:?}, observed {observed:?}",
                    case.retained, case.materialized, case.sealed, case.pin, case.expected
                )
                .into());
            }
        }
    }
    Ok(())
}

#[test]
fn the_unknown_refusal_names_the_pin_and_the_track() -> TestResult {
    let readiness = PinnedGenerationReadinessV1 {
        retained_by_authority: false,
        materialized_head: Some(g(9)),
        sealed_head: Some(g(9)),
    };
    match validate_pinned_generation_v1(
        "lexical",
        &pin(3),
        SearchPlaneTrackKind::Lexical,
        readiness,
    ) {
        Err(CoreError::Typed { code, message }) => {
            if code != UNKNOWN_GENERATION_CODE {
                return Err(format!("wrong code {code}").into());
            }
            for needle in [
                "lexical",
                "generation 3",
                "repo=repo",
                "revision=rev",
                "Lexical",
            ] {
                if !message.contains(needle) {
                    return Err(format!("message lacks `{needle}`: {message}").into());
                }
            }
            Ok(())
        }
        other => Err(format!("expected a typed refusal, got {other:?}").into()),
    }
}
