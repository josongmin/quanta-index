//! Strict symbol capability over the admitted source universe. Query owners
//! supply their existing effective repo/path/language matcher; result predicates
//! and returned hits cannot define this universe.

use quanta_index_contract::{
    FileCoverageSnapshot, SearchPlaneErrorCodeV2, SourceFileCoverage, SymbolCoverage,
};

use crate::{CoreError, RequestBudgetV1};

/// Require completeness for every potentially in-scope admitted file, including
/// zero-unit files and inherited entries. A proved contradictory request scope
/// may be resolved by the validated planner before calling this function; no
/// untrusted `empty` flag can bypass it here.
pub fn require_complete_symbol_coverage(
    snapshot: Option<&FileCoverageSnapshot>,
    mut in_scope: impl FnMut(&SourceFileCoverage) -> Result<bool, CoreError>,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    budget.checkpoint("symbol coverage admission")?;
    let snapshot = snapshot.ok_or_else(|| CoreError::Typed {
        code: SearchPlaneErrorCodeV2::SymbolCoverageUnavailable,
        message: "symbol authority requires generation-bound source-file coverage".into(),
    })?;
    for (key, entry) in snapshot {
        budget.checkpoint("symbol coverage scope scan")?;
        if key != &entry.source.file {
            return Err(CoreError::Storage(
                "source-file coverage key differs from its source identity".into(),
            ));
        }
        entry.validate().map_err(|error| {
            CoreError::Storage(format!("invalid source-file coverage identity: {error}"))
        })?;
        if in_scope(entry)? && !matches!(entry.symbols, SymbolCoverage::Complete { .. }) {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SymbolCoverageIncomplete,
                message: format!(
                    "symbol coverage is {:?} for source repo={} path={}",
                    entry.symbols,
                    key.source_repo_id.as_str(),
                    key.repo_relative_path.as_str(),
                ),
            });
        }
    }
    budget.checkpoint("symbol coverage admission complete")
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "assertions report regression failures"
)]
mod tests {
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        FileCoverageSnapshot, RepoId, RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2,
        SourceFileCoverage, SourceFileKey, SourceFileRevision, SymbolCoverage,
        source_file_unit_set_sha256,
    };

    use super::require_complete_symbol_coverage;
    use crate::{CoreError, RequestBudgetV1};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn file(
        path: &str,
        symbols: SymbolCoverage,
    ) -> Result<SourceFileCoverage, Box<dyn std::error::Error>> {
        Ok(SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source")?,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new("revision")?,
                source_sha256: [1; 32],
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [2; 32],
            unit_set_sha256: source_file_unit_set_sha256(&[], &[])?,
            text_admitted: true,
            symbols,
        })
    }

    fn snapshot(entries: impl IntoIterator<Item = SourceFileCoverage>) -> FileCoverageSnapshot {
        entries
            .into_iter()
            .map(|entry| (entry.source.file.clone(), entry))
            .collect()
    }

    #[test]
    fn only_complete_including_zero_can_claim_strict_symbol_authority() -> TestResult {
        for state in [
            SymbolCoverage::Complete { symbol_count: 0 },
            SymbolCoverage::Complete { symbol_count: 3 },
            SymbolCoverage::NotRequested,
            SymbolCoverage::Unsupported,
            SymbolCoverage::ParseFailed,
            SymbolCoverage::ProducerFailed,
        ] {
            let universe = snapshot([file("a.rs", state)?]);
            let result = require_complete_symbol_coverage(
                Some(&universe),
                |_| Ok(true),
                &RequestBudgetV1::unbounded(),
            );
            if matches!(state, SymbolCoverage::Complete { .. }) {
                result?;
            } else {
                assert!(matches!(
                    result,
                    Err(CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::SymbolCoverageIncomplete,
                        ..
                    })
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn missing_coverage_is_distinct_from_an_admitted_empty_universe() -> TestResult {
        assert!(matches!(
            require_complete_symbol_coverage(None, |_| Ok(false), &RequestBudgetV1::unbounded()),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SymbolCoverageUnavailable,
                ..
            })
        ));
        require_complete_symbol_coverage(
            Some(&FileCoverageSnapshot::new()),
            |_| {
                Err(CoreError::Storage(
                    "empty universe must not invoke matcher".into(),
                ))
            },
            &RequestBudgetV1::unbounded(),
        )?;
        Ok(())
    }

    #[test]
    fn last_admitted_file_failure_rejects_broad_scope_but_not_narrow_scope() -> TestResult {
        let universe = snapshot([
            file("a.rs", SymbolCoverage::Complete { symbol_count: 0 })?,
            file("z.rs", SymbolCoverage::ParseFailed)?,
        ]);
        let mut visited = Vec::new();
        let broad = require_complete_symbol_coverage(
            Some(&universe),
            |entry| {
                visited.push(entry.source.file.repo_relative_path.as_str().to_owned());
                Ok(true)
            },
            &RequestBudgetV1::unbounded(),
        );
        assert!(matches!(
            broad,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SymbolCoverageIncomplete,
                ..
            })
        ));
        assert_eq!(visited, vec!["a.rs", "z.rs"]);
        require_complete_symbol_coverage(
            Some(&universe),
            |entry| Ok(entry.source.file.repo_relative_path.as_str() == "a.rs"),
            &RequestBudgetV1::unbounded(),
        )?;
        Ok(())
    }

    #[test]
    fn corrupt_key_or_source_cannot_be_hidden_by_a_nonmatching_scope() -> TestResult {
        let entry = file("a.rs", SymbolCoverage::Complete { symbol_count: 0 })?;
        let mut key = entry.source.file.clone();
        key.repo_relative_path = RepoRelativePath::new("other.rs");
        let corrupt = FileCoverageSnapshot::from([(key, entry.clone())]);
        assert!(matches!(
            require_complete_symbol_coverage(
                Some(&corrupt),
                |_| Ok(false),
                &RequestBudgetV1::unbounded()
            ),
            Err(CoreError::Storage(_))
        ));
        let mut invalid = entry;
        invalid.source.file.repo_relative_path = RepoRelativePath::new("../escape.rs");
        assert!(matches!(
            require_complete_symbol_coverage(
                Some(&snapshot([invalid])),
                |_| Ok(false),
                &RequestBudgetV1::unbounded()
            ),
            Err(CoreError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn matcher_errors_and_cancellation_never_become_empty_success() -> TestResult {
        let universe = snapshot([file("a.rs", SymbolCoverage::Complete { symbol_count: 0 })?]);
        assert!(matches!(
            require_complete_symbol_coverage(
                Some(&universe),
                |_| { Err(CoreError::InvalidContract("matcher failure".into())) },
                &RequestBudgetV1::unbounded()
            ),
            Err(CoreError::InvalidContract(_))
        ));
        let budget = RequestBudgetV1::unbounded();
        budget.cancel_handle().cancel();
        assert!(matches!(
            require_complete_symbol_coverage(Some(&universe), |_| Ok(true), &budget),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));
        Ok(())
    }
}
