//! Source-bound feature extraction and preregistered diagnostic rank ablations.
//! The selected file scorer remains owned by the parent; study scores are never
//! silently substituted for its output. Symbol evidence is file-level only.

use super::*;
use quanta_index_core::CodeSearchRankStudyV1;

// Initial experimental weights; evaluate on development data before freezing
// a policy for holdout. These are not copied Zoekt constants or tuned Gin gold.
const DECLARATION_EXACT: u32 = 64;
const DECLARATION_EDGE: u32 = 32;
const DECLARATION_INFIX: u32 = 16;

fn subtoken_edge(raw: &str, at: usize) -> bool {
    let before = raw.get(..at).and_then(|s| s.chars().next_back());
    let mut after = raw.get(at..).into_iter().flat_map(str::chars);
    let current = after.next();
    let next = after.next();
    match (before, current) {
        (Some(left), Some(right)) => {
            (left == '_' && right.is_ascii_alphanumeric())
                || (right == '_' && left.is_ascii_alphanumeric())
                || (left.is_ascii_lowercase() && right.is_ascii_uppercase())
                || (left.is_ascii_alphabetic() && right.is_ascii_digit())
                || (left.is_ascii_digit() && right.is_ascii_alphabetic())
                || (left.is_ascii_uppercase()
                    && right.is_ascii_uppercase()
                    && next.is_some_and(|ch| ch.is_ascii_lowercase()))
        }
        _ => false,
    }
}

fn original_boundary_bonus(raw: &str, span: Range<usize>) -> Result<u32, CoreError> {
    if span.is_empty() || raw.get(span.clone()).is_none() {
        return Err(CoreError::Storage(
            "lexical: rank feature source span is invalid".into(),
        ));
    }
    let left_inner = subtoken_edge(raw, span.start);
    let right_inner = subtoken_edge(raw, span.end);
    if !left_inner && !right_inner {
        return Ok(0);
    }
    let before = raw.get(..span.start).and_then(|s| s.chars().next_back());
    let after = raw.get(span.end..).and_then(|s| s.chars().next());
    let left = left_inner || before.is_none_or(|ch| !normalize::is_token_char(ch));
    let right = right_inner || after.is_none_or(|ch| !normalize::is_token_char(ch));
    Ok(if left && right { 16 } else { 8 })
}

fn boundary_features(
    owner: &TantivySearcher,
    file: &SourceFile,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    budget: &RequestBudgetV1,
) -> Result<u32, CoreError> {
    let surfaces = [
        (
            Scope::Content,
            std::str::from_utf8(&file.bytes).ok(),
            file.indexed_text.as_deref(),
        ),
        (
            Scope::Path,
            Some(file.source.file.repo_relative_path.as_str()),
            Some(file.indexed_path.as_str()),
        ),
    ];
    let mut per_term = vec![0_u32; terms.len()];
    for (scope, raw, indexed) in surfaces {
        let (Some(raw), Some(indexed)) = (raw, indexed) else {
            continue;
        };
        let admitted: Vec<_> = terms
            .iter()
            .enumerate()
            .filter(|(_, term)| {
                term.regex.is_none() && matches!(term.scope, Scope::Both)
                    || term.regex.is_none()
                        && match (term.scope, scope) {
                            (Scope::Content, Scope::Content) | (Scope::Path, Scope::Path) => true,
                            _ => false,
                        }
            })
            .collect();
        if admitted.is_empty() {
            continue;
        }
        let max_entries = raw
            .len()
            .checked_mul(8)
            .ok_or_else(|| CoreError::Storage("lexical: rank provenance size overflow".into()))?
            .max(1);
        let max_bytes = raw.len().max(indexed.len()).saturating_mul(3);
        let allocation = if raw.is_ascii() {
            0
        } else {
            MappedText::allocation_bound(max_bytes, max_entries).ok_or_else(|| {
                CoreError::Storage("lexical: rank provenance allocation overflow".into())
            })?
        };
        let resources = owner.execution_budget.collection_budget(1)?;
        let _lease =
            resources.reserve_bytes(u64::try_from(allocation).map_err(|error| {
                CoreError::Storage(format!("lexical: rank allocation: {error}"))
            })?)?;
        let mapped = if raw.is_ascii() {
            None
        } else {
            Some(
                MappedText::new(raw, indexed, case, max_bytes, max_entries, &|| {
                    budget.interruption().is_some()
                })
                .map_err(|error| {
                    budget
                        .interrupted_at("lexical:rank-original-boundary")
                        .unwrap_or_else(|| {
                            CoreError::Storage(format!(
                                "lexical: rank original-byte provenance: {error}"
                            ))
                        })
                })?,
            )
        };
        let text = match scope {
            Scope::Content => match case {
                CaseMode::Sensitive => indexed,
                CaseMode::Folded => file.folded_text.as_deref().ok_or_else(|| {
                    CoreError::Storage("lexical: rank source lacks folded text".into())
                })?,
            },
            Scope::Path => match case {
                CaseMode::Sensitive => &file.indexed_path,
                CaseMode::Folded => &file.folded_path,
            },
            Scope::Both => return Err(CoreError::Storage("lexical: invalid rank surface".into())),
        };
        for (index, term) in admitted {
            for start in OverlappingMatches::new(text, &term.needle) {
                budget.checkpoint("lexical:rank-original-boundary")?;
                let span = start..start.saturating_add(term.needle.len());
                let original = match &mapped {
                    Some(mapped) => mapped.source_range(span).map_err(|error| {
                        CoreError::Storage(format!("lexical: rank source mapping: {error}"))
                    })?,
                    None => span,
                };
                let value = original_boundary_bonus(raw, original)?;
                let prior = per_term.get_mut(index).ok_or_else(|| {
                    CoreError::Storage("lexical: rank term index is invalid".into())
                })?;
                *prior = (*prior).max(value);
            }
        }
    }
    Ok(per_term.into_iter().fold(0_u32, u32::saturating_add))
}

fn stored_text<'a>(
    doc: &'a TantivyDocument,
    field: tantivy::schema::Field,
) -> Result<&'a str, CoreError> {
    let mut values = doc.get_all(field);
    let value = values
        .next()
        .and_then(|value| value.as_str())
        .ok_or_else(|| CoreError::Storage("lexical: declaration feature lacks name".into()))?;
    if values.next().is_some() {
        return Err(CoreError::Storage(
            "lexical: duplicate declaration feature name".into(),
        ));
    }
    Ok(value)
}

fn stored_offset(doc: &TantivyDocument, field: tantivy::schema::Field) -> Result<usize, CoreError> {
    let mut values = doc.get_all(field);
    let value = values
        .next()
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            CoreError::Storage("lexical: declaration feature lacks definition range".into())
        })?;
    if values.next().is_some() {
        return Err(CoreError::Storage(
            "lexical: duplicate declaration feature range".into(),
        ));
    }
    usize::try_from(value)
        .map_err(|error| CoreError::Storage(format!("lexical: declaration range: {error}")))
}

fn declaration_features(
    owner: &TantivySearcher,
    file: &SourceFile,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    budget: &RequestBudgetV1,
) -> Result<(Option<u32>, bool), CoreError> {
    let coverage = owner
        .source_coverage
        .as_ref()
        .and_then(|rows| rows.get(&file.source.file));
    if coverage.is_some_and(|row| row.source != file.source) {
        return Err(CoreError::Storage(
            "lexical: rank declaration coverage has stale source".into(),
        ));
    }
    let mut complete =
        coverage.is_some_and(|row| matches!(row.symbols, SymbolCoverage::Complete { .. }));
    if !terms
        .iter()
        .any(|term| term.regex.is_none() && !matches!(term.scope, Scope::Path))
    {
        return Ok((complete.then_some(0), complete));
    }
    let query = BooleanQuery::new(vec![
        (
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(owner.fields.doc_kind, crate::SYMBOL_DOC_KIND),
                IndexRecordOption::Basic,
            )),
        ),
        (
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(
                    owner.fields.repo_id,
                    file.source.file.source_repo_id.as_str(),
                ),
                IndexRecordOption::Basic,
            )),
        ),
        (
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(
                    owner.fields.repo_relative_path,
                    file.source.file.repo_relative_path.as_str(),
                ),
                IndexRecordOption::Basic,
            )),
        ),
    ]);
    let searcher = owner.reader.searcher();
    let rows = owner.collect_whole_set(
        &searcher,
        &query,
        1.0,
        "code-search rank declarations",
        budget,
    )?;
    let mut per_term = vec![0_u32; terms.len()];
    for row in rows.iter() {
        budget.checkpoint("lexical:rank-declaration-source")?;
        let doc = searcher
            .doc::<TantivyDocument>(row.address)
            .map_err(|error| {
                CoreError::Storage(format!("lexical: declaration feature document: {error}"))
            })?;
        let symbol = owner.document_to_symbol_candidate_identity(&doc, 0.0)?;
        if symbol.source.as_ref() != Some(&file.source) {
            return Err(CoreError::Storage(
                "lexical: rank declaration identity has stale source".into(),
            ));
        }
        let name = stored_text(&doc, owner.fields.symbol_local_name_original)?;
        let start = stored_offset(&doc, owner.fields.symbol_definition_start_byte)?;
        let end = stored_offset(&doc, owner.fields.symbol_definition_end_byte)?;
        let source = file.bytes.get(start..end).ok_or_else(|| {
            CoreError::Storage("lexical: rank declaration range is outside source".into())
        })?;
        if name.is_empty() {
            return Err(CoreError::Storage(
                "lexical: rank declaration name is empty".into(),
            ));
        }
        if memchr::memmem::find(source, name.as_bytes()).is_none() {
            if name.is_ascii()
                && coverage.is_some_and(|row| {
                    row.symbol_name_source_policy
                        == quanta_index_contract::SymbolNameSourcePolicyV1::RawAsciiLocalName
                })
            {
                return Err(CoreError::Storage(
                    "lexical: promised declaration name is absent from source range".into(),
                ));
            }
            // Unspecified/non-ASCII producer names may legitimately differ
            // from source spelling. Keep the selected score and expose unknown
            // evidence; never treat their unverified absence as a zero feature.
            complete = false;
            continue;
        }
        let name = normalize::nfc(name);
        let name = normalize::apply_case(name.as_ref(), case);
        for (index, term) in terms.iter().enumerate() {
            if term.regex.is_some() || matches!(term.scope, Scope::Path) {
                continue;
            }
            let Some(at) = name.find(&term.needle) else {
                continue;
            };
            let bonus = if name.as_ref() == term.needle {
                DECLARATION_EXACT
            } else if at == 0 || name.ends_with(&term.needle) {
                DECLARATION_EDGE
            } else {
                DECLARATION_INFIX
            };
            let prior = per_term.get_mut(index).ok_or_else(|| {
                CoreError::Storage("lexical: rank declaration term is invalid".into())
            })?;
            *prior = (*prior).max(bonus);
        }
    }
    let bonus = per_term.into_iter().fold(0_u32, u32::saturating_add);
    Ok((
        if complete || bonus > 0 {
            Some(bonus)
        } else {
            None
        },
        complete,
    ))
}

pub(super) fn study(
    owner: &TantivySearcher,
    file: &SourceFile,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    components: CodeSearchScoreComponentsV1,
    budget: &RequestBudgetV1,
) -> Result<CodeSearchRankStudyV1, CoreError> {
    let (declaration_bonus, complete) = declaration_features(owner, file, terms, case, budget)?;
    let boundary = boundary_features(owner, file, terms, case, budget)?;
    let baseline = components.total();
    let without_occurrences = baseline.saturating_sub(components.occurrence);
    let declaration = declaration_bonus.unwrap_or(0);
    Ok(CodeSearchRankStudyV1 {
        declaration_bonus,
        declaration_coverage_complete: complete,
        original_boundary_bonus: boundary,
        baseline,
        declaration_only: baseline.saturating_add(declaration),
        boundary_only: baseline.saturating_add(boundary),
        occurrence_half: without_occurrences.saturating_add(components.occurrence / 2),
        occurrence_none: without_occurrences,
        combined: without_occurrences
            .saturating_add(declaration)
            .saturating_add(boundary),
    })
}

#[cfg(test)]
mod tests {
    use super::original_boundary_bonus;

    #[test]
    fn original_source_boundaries_follow_independent_camel_snake_acronym_digit_examples() {
        for (raw, needle, expected) in [
            ("fooBar", "Bar", 16),
            ("foo_bar", "bar", 16),
            ("HTTPServer", "Server", 16),
            ("HTTPServer", "HTTP", 16),
            ("parse2Value", "2", 16),
            ("fooBarSuffix", "Bar", 16),
            ("foobar", "bar", 0),
            ("fooBar", "ar", 0),
            ("needle", "needle", 0),
            ("éNeedle", "Needle", 0),
        ] {
            let start = raw.find(needle).expect("fixed source match");
            assert_eq!(
                original_boundary_bonus(raw, start..start + needle.len()).expect("valid span"),
                expected,
                "{raw}/{needle}"
            );
        }
    }
}
