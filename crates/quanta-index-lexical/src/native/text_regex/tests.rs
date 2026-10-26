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

use super::{
    CaseMode, FilePathScope, MAX_NATIVE_CONTENT_REGEX, NativeRegexProgram, PathConstraint,
};

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
    if meter.0.load(Ordering::SeqCst) <= super::ENGINE_LOGICAL_BYTES {
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
        ("tests/main.rs", "DELTA", (0b10, false)),
        ("src/empty.rs", "", (0b00, true)),
    ] {
        let actual = program.evaluate_overlay(path, body, &mut work)?;
        if actual != gold {
            return Err(IndexError::BackendViolation(
                "overlay regex fixed body/path gold differs".into(),
            ));
        }
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
            program.evaluate_overlay(path, "alpha", &mut budget(u64::MAX)?),
            Err(IndexError::InvalidInput(_))
        ) {
            return Err(IndexError::BackendViolation(
                "invalid overlay path reached regex verifier".into(),
            ));
        }
    }
    if !matches!(
        program.evaluate_overlay("src/main.rs", "alpha", &mut budget(1)?),
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
    let over_width = ["a"; MAX_NATIVE_CONTENT_REGEX + 1];
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
        program.evaluate_overlay("src/empty.rs", "", &mut budget(0)?),
        Err(IndexError::ResourceLimited)
    ) {
        return Err(IndexError::BackendViolation(
            "empty regex verification escaped the original work bound".into(),
        ));
    }
    Ok(())
}
