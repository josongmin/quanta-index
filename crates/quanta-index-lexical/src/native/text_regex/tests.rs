use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    WorkBudget, WorkLimits,
};

use super::{CaseMode, FilePathScope, NativeRegexProgram, PathConstraint, TextOverlaySource};

/// An overlay row without repository or language metadata.
fn overlay<'a>(path: &'a str, body: &'a str) -> TextOverlaySource<'a> {
    TextOverlaySource {
        owner: quanta_index_core::index::OwnerKey {
            producer: quanta_index_core::index::ProducerId([0; 16]),
            stable_id: quanta_index_core::index::StableOwnerId([0; 16]),
        },
        revision: 1,
        path,
        body,
        repository: None,
        language: None,
    }
}

fn budget(bytes: u64) -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: u64::MAX,
            writes: u64::MAX,
            bytes,
            mutations: u64::MAX,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

struct Meter(AtomicU64);

impl MemoryLeaseOwner for Meter {
    fn release(&self, bytes: u64) {
        let _prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
    }
}

struct MeterScope(Arc<Meter>);

impl MemoryScopePort for MeterScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _prior = self.0.as_ref().0.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

#[test]
fn regex_program_keeps_hir_and_engine_grants_until_drop() -> Result<(), IndexError> {
    let meter = Arc::new(Meter(AtomicU64::new(0)));
    let mut work = budget(u64::MAX)?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
    let empty = NativeRegexProgram::compile(&[], &[], CaseMode::Sensitive, &mut work)?;
    if meter.0.load(Ordering::SeqCst) != 0 {
        return Err(IndexError::BackendViolation(
            "empty regex program reserved nonzero memory".into(),
        ));
    }
    drop(empty);
    let program = NativeRegexProgram::compile(&["alpha"], &[], CaseMode::Sensitive, &mut work)?;
    let retained = program
        .content
        .first()
        .map(|regex| regex.executor.retained_bytes())
        .ok_or_else(|| IndexError::BackendViolation("content executor missing".into()))?;
    if meter.0.load(Ordering::SeqCst)
        < u64::try_from(retained).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::BackendViolation(
            "regex HIR or pattern grant ended before executor".into(),
        ));
    }
    drop(program);
    if meter.0.load(Ordering::SeqCst) != 0 {
        return Err(IndexError::BackendViolation(
            "regex grant outlived executor".into(),
        ));
    }
    Ok(())
}

#[test]
fn content_regex_uses_canonical_case_and_nfc() -> Result<(), IndexError> {
    let folded =
        NativeRegexProgram::compile(&["café"], &[], CaseMode::Folded, &mut budget(u64::MAX)?)?;
    let folded_regex = folded.content.first().ok_or_else(|| {
        IndexError::BackendViolation("folded regex executor was not constructed".into())
    })?;
    if !folded_regex.executor.verify("CAFÉ".as_bytes()) {
        return Err(IndexError::BackendViolation(
            "folded regex fixed gold differs".into(),
        ));
    }
    let sensitive = NativeRegexProgram::compile(
        &["cafe\u{301}"],
        &[],
        CaseMode::Sensitive,
        &mut budget(u64::MAX)?,
    )?;
    let sensitive_regex = sensitive.content.first().ok_or_else(|| {
        IndexError::BackendViolation("sensitive regex executor was not constructed".into())
    })?;
    if !sensitive_regex.executor.verify("café".as_bytes())
        || sensitive_regex.executor.verify("CAFÉ".as_bytes())
    {
        return Err(IndexError::BackendViolation(
            "sensitive NFC regex fixed gold differs".into(),
        ));
    }
    Ok(())
}

#[test]
fn overlay_regex_matches_nfc_body_and_authoritative_path() -> Result<(), IndexError> {
    let path = PathConstraint {
        pattern: "^src/.*\\.rs$".into(),
        scope: FilePathScope::PathOnly,
    };
    let mut work = budget(u64::MAX)?;
    let program =
        NativeRegexProgram::compile(&["café", "delta"], &[path], CaseMode::Folded, &mut work)?;
    for (path, body, gold) in [
        ("src/main.rs", "CAFE\u{301}", (0b01, true)),
        ("tests/main.rs", "DELTA", (0b00, false)),
        ("src/empty.rs", "", (0b00, true)),
    ] {
        let actual = program
            .evaluate_overlay(&overlay(path, body), None, &mut work)
            .map(|(mask, path_match, _ranked)| (mask, path_match))?;
        if actual != gold {
            return Err(IndexError::BackendViolation(
                "overlay regex fixed body/path gold differs".into(),
            ));
        }
    }
    let body = "DELTA".repeat(1_024);
    if program.evaluate_overlay("tests/main.rs", &body, &mut budget(4_096)?)? != (0, false) {
        return Err(IndexError::BackendViolation(
            "a rejected path must never consume the body scan budget".into(),
        ));
    }
    Ok(())
}

#[test]
fn overlay_regex_refuses_invalid_shape_and_scan_budget() -> Result<(), IndexError> {
    let mut compile_work = budget(u64::MAX)?;
    let program =
        NativeRegexProgram::compile(&["alpha"], &[], CaseMode::Sensitive, &mut compile_work)?;
    for path in ["", "/root.rs", "src/../main.rs", "src//main.rs"] {
        if !matches!(
            program.evaluate_overlay(&overlay(path, "alpha"), None, &mut budget(u64::MAX)?),
            Err(IndexError::InvalidInput(_))
        ) {
            return Err(IndexError::BackendViolation(
                "invalid overlay path reached regex verifier".into(),
            ));
        }
    }
    if !matches!(
        program.evaluate_overlay(&overlay("src/main.rs", "alpha"), None, &mut budget(1)?),
        Err(IndexError::ResourceLimited)
    ) {
        return Err(IndexError::BackendViolation(
            "overlay regex scan bypassed original budget".into(),
        ));
    }
    Ok(())
}

#[test]
fn path_filter_matches_authoritative_full_path_or_basename() -> Result<(), IndexError> {
    let path_only = PathConstraint {
        pattern: "^src/.*\\.rs$".into(),
        scope: FilePathScope::PathOnly,
    };
    let basename = PathConstraint {
        pattern: "^main\\.rs$".into(),
        scope: FilePathScope::NameAndPath,
    };
    let mut work = budget(u64::MAX)?;
    let matched =
        NativeRegexProgram::compile(&[], &[path_only, basename], CaseMode::Folded, &mut work)?;
    if !matched.matches_path("src/main.rs", &mut work)?
        || matched.matches_path("test/main.rs", &mut work)?
        || matched.matches_path("src/lib.rs", &mut work)?
    {
        return Err(IndexError::BackendViolation(
            "path regex fixed gold differs".into(),
        ));
    }
    Ok(())
}

#[test]
fn bounded_regex_refuses_width_and_original_budget_before_engine() -> Result<(), IndexError> {
    let over_width = ["a"; quanta_index_contract::MAX_NATIVE_CONTENT_REGEX_LEAVES + 1];
    if !matches!(
        NativeRegexProgram::compile(
            &over_width,
            &[],
            CaseMode::Sensitive,
            &mut budget(u64::MAX)?
        ),
        Err(IndexError::ResourceLimited)
    ) {
        return Err(IndexError::BackendViolation(
            "regex width cap was bypassed".into(),
        ));
    }
    let mut limited = budget(16)?;
    if !matches!(
        NativeRegexProgram::compile(&["alpha"], &[], CaseMode::Sensitive, &mut limited),
        Err(IndexError::ResourceLimited)
    ) {
        return Err(IndexError::BackendViolation(
            "regex original budget was bypassed".into(),
        ));
    }
    Ok(())
}

#[test]
fn empty_overlay_body_still_charges_regex_verification() -> Result<(), IndexError> {
    let program =
        NativeRegexProgram::compile(&["^$"], &[], CaseMode::Sensitive, &mut budget(u64::MAX)?)?;
    if !matches!(
        program.evaluate_overlay(&overlay("src/empty.rs", ""), None, &mut budget(0)?),
        Err(IndexError::ResourceLimited)
    ) {
        return Err(IndexError::BackendViolation(
            "empty regex verification escaped the original work bound".into(),
        ));
    }
    Ok(())
}

/// Far below one document engine's two lazy-DFA cache capacities.
const REPO_FILTERS_CEILING: u64 = 1 << 20;

/// A `repo:` engine searches one repository id, so it is a field engine
/// charged its own measured retained heap. Four filters, the most one query
/// may carry, stay below [`REPO_FILTERS_CEILING`], and every grant ends with
/// the program.
#[test]
fn repo_filters_charge_their_measured_field_engines() -> Result<(), IndexError> {
    use crate::filters::RepoConstraint;
    for patterns in [
        &["corp-a"][..],
        &["corp", "^corp-a$", "a$", "c.rp-[a-z]"],
        &[r"\w+", r"(?i)corp-\w+", r"^.*$", r"[\p{L}\d]+-\w+"],
    ] {
        let meter = Arc::new(Meter(AtomicU64::new(0)));
        let mut work = budget(u64::MAX)?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
        let constraints: Vec<RepoConstraint> = patterns
            .iter()
            .map(|pattern| RepoConstraint {
                pattern: (*pattern).into(),
                revs: Vec::new(),
            })
            .collect();
        let program = NativeRegexProgram::compile(&[], &[], CaseMode::Sensitive, &mut work)?
            .with_record_metadata(&constraints, &[], &mut work)?;
        let reserved = meter.0.load(Ordering::SeqCst);
        let retained = program
            .metadata
            .repositories
            .iter()
            .map(|regex| regex.executor.retained_bytes())
            .try_fold(0_u64, |total, bytes| {
                u64::try_from(bytes)
                    .ok()
                    .and_then(|bytes| total.checked_add(bytes))
            })
            .ok_or(IndexError::ResourceLimited)?;
        // Every engine holds at least its retained heap, and together they
        // stay below the ceiling.
        if reserved < retained
            || reserved >= REPO_FILTERS_CEILING
            || work.usage().bytes >= REPO_FILTERS_CEILING
        {
            return Err(IndexError::BackendViolation(format!(
                "{patterns:?} reserved {reserved} and charged {} bytes",
                work.usage().bytes
            )));
        }
        let corp_a = quanta_index_contract::RepoId::new("corp-a")
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        if !program.matches_metadata(Some(&corp_a), None, &mut work)? {
            return Err(IndexError::BackendViolation(format!(
                "{patterns:?} did not match corp-a"
            )));
        }
        drop(program);
        if meter.0.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "repo filter grant outlived its program".into(),
            ));
        }
    }
    Ok(())
}

/// A `file:` filter searches one short path or file name, so it is a field
/// engine charged its own measured heap. Four filters, the most one query may
/// carry, stay below [`REPO_FILTERS_CEILING`], match like before, and every
/// grant ends with the program.
#[test]
fn path_filters_charge_their_measured_field_engines() -> Result<(), IndexError> {
    let path = |pattern: &str, scope| PathConstraint {
        pattern: pattern.into(),
        scope,
    };
    let filters = [
        path("^src/", FilePathScope::PathOnly),
        path(r"[.]rs$", FilePathScope::NameAndPath),
        path("^main", FilePathScope::NameOnly),
        path(r"(?i)\w+/MAIN", FilePathScope::PathOnly),
    ];
    let meter = Arc::new(Meter(AtomicU64::new(0)));
    let mut work = budget(u64::MAX)?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
    let program = NativeRegexProgram::compile(&[], &filters, CaseMode::Sensitive, &mut work)?;
    let reserved = meter.0.load(Ordering::SeqCst);
    if reserved >= REPO_FILTERS_CEILING || work.usage().bytes >= REPO_FILTERS_CEILING {
        return Err(IndexError::BackendViolation(format!(
            "four path filters reserved {reserved} and charged {} bytes",
            work.usage().bytes
        )));
    }
    for (candidate, expected) in [
        ("src/main.rs", true),
        ("src/lib.rs", false),
        ("tests/main.rs", false),
        ("src/nested/main.rs", true),
    ] {
        if program.matches_path_filters(candidate, &mut work)? != expected {
            return Err(IndexError::BackendViolation(format!(
                "path filters disagree on {candidate:?}"
            )));
        }
    }
    drop(program);
    if meter.0.load(Ordering::SeqCst) != 0 {
        return Err(IndexError::BackendViolation(
            "path filter grant outlived its program".into(),
        ));
    }
    Ok(())
}
