//! Custodial lowering for the Native Text planner.

use quanta_index_contract::{
    LqFilter, LqQuery, MAX_NATIVE_CONTENT_INPUT_BYTES, TextQueryRequest, TextQuerySyntax,
    code_search_query_backing_bytes, lower_code_search_native_query,
    native_content_query_backing_bytes, parse_sourcegraph_content_query,
    sourcegraph_query_backing_bytes, tokenize_native_content_query,
};
use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime};

/// The parser's owned query and the retained grant acquired before lexing.
/// Callers keep this value live until the plan has adopted its own grant.
pub(crate) struct LoweredNativeText {
    pub(crate) query: LqQuery,
    backing: Option<MemoryLease>,
}

impl LoweredNativeText {
    pub(crate) fn into_parts(self) -> (LqQuery, Option<MemoryLease>) {
        (self.query, self.backing)
    }
}

/// Lower the request's content grammar with a grant before parsing.
///
/// The Native LQ parser pre-reserves regex HIR. The Sourcegraph bridge and the
/// shared product code-search grammar lower regex syntax to an AST leaf and
/// the planner funds its preparation. The top-level language constraint
/// becomes per-file `lang:` filters on the lowered query.
pub(crate) fn lower_native_text_input(
    request: &TextQueryRequest,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<LoweredNativeText, IndexError> {
    if request.cursor.is_some() {
        return Err(IndexError::InvalidInput(
            "native text lowering requires a first-page query".into(),
        ));
    }
    let input = request.query_text.as_str();
    if input.len() > MAX_NATIVE_CONTENT_INPUT_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let backing_bytes = match request.syntax {
        TextQuerySyntax::Native => native_content_query_backing_bytes(input.len()),
        TextQuerySyntax::Sourcegraph => sourcegraph_query_backing_bytes(input.len()),
        TextQuerySyntax::CodeSearch => code_search_query_backing_bytes(input.len()),
    };
    let backing = reserve(
        backing_bytes
            .ok_or(IndexError::ResourceLimited)?
            .checked_add(language_filter_backing_bytes(request)?)
            .ok_or(IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    if backing.is_none() {
        return Err(IndexError::BackendViolation(
            "native text lowering requires allocation custody".into(),
        ));
    }
    let ast_only = match request.syntax {
        TextQuerySyntax::Sourcegraph => Some(
            parse_sourcegraph_content_query(input)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?,
        ),
        TextQuerySyntax::CodeSearch => Some(
            lower_code_search_native_query(input)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?,
        ),
        TextQuerySyntax::Native => None,
    };
    if let Some(mut query) = ast_only {
        lower_language_constraint(&mut query, request)?;
        return Ok(LoweredNativeText { query, backing });
    }
    let tokens = tokenize_native_content_query(input)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let preparation_bytes = tokens
        .preparation_backing_bytes()
        .ok_or(IndexError::ResourceLimited)?;
    // The parser's regex HIR lives only while it validates the pattern; the
    // parsed query keeps the pattern text, which the query grant covers.
    let preparation = if preparation_bytes == 0 {
        None
    } else {
        let memory = reserve(preparation_bytes, MemoryLifetime::Temporary)?;
        if memory.is_none() {
            return Err(IndexError::BackendViolation(
                "native regex preparation requires allocation custody".into(),
            ));
        }
        memory
    };
    let mut query = tokens
        .parse()
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    drop(preparation);
    lower_language_constraint(&mut query, request)?;
    Ok(LoweredNativeText { query, backing })
}

/// Logical backing for the `lang:` filters lowered from the constraint.
fn language_filter_backing_bytes(request: &TextQueryRequest) -> Result<u64, IndexError> {
    let bytes = request
        .constraints
        .language_any_of
        .iter()
        .try_fold(0_usize, |total, language| {
            total
                .checked_add(language.as_str().len())?
                .checked_add(std::mem::size_of::<LqFilter>())
        })
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

/// Select only Sources whose verified language is in the constraint.
///
/// Several `lang:` filters are an any-of set. A query that already names
/// `lang:` cannot also carry the constraint, because the two sets would widen
/// each other instead of intersecting.
fn lower_language_constraint(
    query: &mut LqQuery,
    request: &TextQueryRequest,
) -> Result<(), IndexError> {
    let languages = &request.constraints.language_any_of;
    if languages.is_empty() {
        return Ok(());
    }
    if query
        .filters
        .iter()
        .any(|filter| matches!(filter, LqFilter::Lang { .. }))
    {
        return Err(IndexError::InvalidInput(
            "native text language constraint cannot combine with a lang: filter".into(),
        ));
    }
    query
        .filters
        .try_reserve_exact(languages.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for language in languages {
        query.filters.push(LqFilter::Lang {
            id: language.as_str().to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    use quanta_index_contract::{LqExpr, QueryConstraintSetV1};
    use quanta_index_core::index::MemoryLeaseOwner;

    use super::*;

    struct Releases(AtomicU64);

    impl MemoryLeaseOwner for Releases {
        fn release(&self, bytes: u64) {
            let _prior = self.0.fetch_add(bytes, Ordering::SeqCst);
        }
    }

    fn request(text: &str) -> TextQueryRequest {
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: text.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        }
    }

    #[test]
    fn regex_preparation_is_admitted_before_parser_and_released() {
        let owner = Arc::new(Releases(AtomicU64::new(0)));
        let admitted = AtomicU64::new(0);
        let temporary = AtomicU64::new(0);
        let result = lower_native_text_input(&request(r"/\p{L}/"), |bytes, lifetime| {
            if lifetime == MemoryLifetime::Temporary {
                let _prior = temporary.fetch_add(bytes, Ordering::SeqCst);
            }
            let _prior = admitted.fetch_add(bytes, Ordering::SeqCst);
            Ok(Some(MemoryLease::new(bytes, owner.clone())))
        });
        assert!(result.is_ok());
        // The regex preparation grant ends with parsing; only the query's
        // own grant outlives lowering.
        assert!(temporary.load(Ordering::SeqCst) > 0);
        assert_eq!(
            owner.0.load(Ordering::SeqCst),
            temporary.load(Ordering::SeqCst)
        );
        drop(result);
        assert!(admitted.load(Ordering::SeqCst) > 0);
        assert_eq!(
            owner.0.load(Ordering::SeqCst),
            admitted.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn quoted_regex_text_is_a_phrase_and_keeps_grant_until_drop() {
        let owner = Arc::new(Releases(AtomicU64::new(0)));
        let request = request(r#""/regex/""#);
        let lowered = lower_native_text_input(&request, |bytes, lifetime| {
            assert_eq!(lifetime, MemoryLifetime::Retained);
            Ok(Some(MemoryLease::new(bytes, owner.clone())))
        });
        let lowered = lowered.expect("quoted text should lower as phrase");
        assert!(matches!(
            &lowered.query.expr,
            LqExpr::Leaf(quanta_index_contract::LqLeaf::Phrase(_))
        ));
        assert_eq!(owner.0.load(Ordering::SeqCst), 0);
        let (_query, backing) = lowered.into_parts();
        let bytes: u64 = backing.iter().map(MemoryLease::bytes).sum();
        drop(backing);
        assert_eq!(owner.0.load(Ordering::SeqCst), bytes);
    }

    #[test]
    fn raw_structural_and_predicate_tokens_refuse_before_parser() {
        for source in ["'raw'", "match { foo }", "symbol.has.name(\"foo\")"] {
            let result = lower_native_text_input(&request(source), |bytes, _lifetime| {
                Ok(Some(MemoryLease::new(
                    bytes,
                    Arc::new(Releases(AtomicU64::new(0))),
                )))
            });
            assert!(matches!(
                result,
                Err(IndexError::InvalidInput(message)) if message.contains("native BM25")
            ));
        }
    }

    #[test]
    fn malformed_input_is_typed_and_oversize_refuses_before_reservation() {
        let malformed = lower_native_text_input(&request("\"unterminated"), |bytes, _lifetime| {
            Ok(Some(MemoryLease::new(
                bytes,
                Arc::new(Releases(AtomicU64::new(0))),
            )))
        });
        assert!(matches!(malformed, Err(IndexError::InvalidInput(_))));

        let oversized = request(&"a".repeat(MAX_NATIVE_CONTENT_INPUT_BYTES + 1));
        let reservation_calls = AtomicU64::new(0);
        let refused = lower_native_text_input(&oversized, |_bytes, _lifetime| {
            let _prior = reservation_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        });
        assert!(matches!(refused, Err(IndexError::ResourceLimited)));
        assert_eq!(reservation_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn sourcegraph_bridge_runs_after_retained_admission() {
        let mut request = request("path:^src/lib[.]rs$ needle");
        request.syntax = TextQuerySyntax::Sourcegraph;
        assert!(matches!(
            lower_native_text_input(&request, |_, _| Err(IndexError::ResourceLimited)),
            Err(IndexError::ResourceLimited)
        ));
        let owner = Arc::new(Releases(AtomicU64::new(0)));
        let lowered = lower_native_text_input(&request, |bytes, lifetime| {
            assert_eq!(lifetime, MemoryLifetime::Retained);
            Ok(Some(MemoryLease::new(bytes, owner.clone())))
        });
        let lowered = lowered.expect("bounded Sourcegraph request must lower");
        assert!(matches!(
            lowered.query.filters.as_slice(),
            [quanta_index_contract::LqFilter::File { pattern, .. }] if pattern == "^src/lib[.]rs$"
        ));
        assert_eq!(owner.0.load(Ordering::SeqCst), 0);
        drop(lowered);
        assert!(owner.0.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn code_search_lowers_through_the_shared_grammar_after_retained_admission() {
        let mut code_search = request(r#"WithContext path:src/Lib.rs "hello brave world""#);
        code_search.syntax = TextQuerySyntax::CodeSearch;
        assert!(matches!(
            lower_native_text_input(&code_search, |_, _| Err(IndexError::ResourceLimited)),
            Err(IndexError::ResourceLimited)
        ));
        let owner = Arc::new(Releases(AtomicU64::new(0)));
        let lowered = lower_native_text_input(&code_search, |bytes, lifetime| {
            assert_eq!(lifetime, MemoryLifetime::Retained);
            Ok(Some(MemoryLease::new(bytes, owner.clone())))
        });
        let lowered = lowered.expect("bounded code search must lower");
        // A `path:` literal is a path substring leaf of the conjunction.
        let path_leaf =
            quanta_index_contract::LqExpr::Leaf(quanta_index_contract::LqLeaf::Predicate {
                name: "path.contains".into(),
                args: vec![quanta_index_contract::LqPredicateArg::RawString(
                    "src/Lib.rs".into(),
                )],
            });
        assert!(matches!(
            &lowered.query.expr,
            quanta_index_contract::LqExpr::All(children) if children.contains(&path_leaf)
        ));
        assert!(
            lowered
                .query
                .filters
                .contains(&quanta_index_contract::LqFilter::Select {
                    dim: quanta_index_contract::LqSelect::Path,
                })
        );
        assert_eq!(owner.0.load(Ordering::SeqCst), 0);
        drop(lowered);
        assert!(owner.0.load(Ordering::SeqCst) > 0);

        for refused in ["typo:load_jsom", "a b c d e", "lang:rust needle"] {
            let mut refused = request(refused);
            refused.syntax = TextQuerySyntax::CodeSearch;
            assert!(matches!(
                lower_native_text_input(&refused, |bytes, _lifetime| {
                    Ok(Some(MemoryLease::new(
                        bytes,
                        Arc::new(Releases(AtomicU64::new(0))),
                    )))
                }),
                Err(IndexError::InvalidInput(message)) if message.starts_with("code_search: ")
            ));
        }
    }

    #[test]
    fn language_constraint_lowers_to_any_of_lang_filters_once() {
        let reserve = |bytes, _lifetime| {
            Ok(Some(MemoryLease::new(
                bytes,
                Arc::new(Releases(AtomicU64::new(0))),
            )))
        };
        let languages = [
            quanta_index_contract::lex::LanguageCode::new("rust").expect("valid language"),
            quanta_index_contract::lex::LanguageCode::new("markdown").expect("valid language"),
        ];
        for syntax in [
            TextQuerySyntax::CodeSearch,
            TextQuerySyntax::Native,
            TextQuerySyntax::Sourcegraph,
        ] {
            let mut constrained = request("needle");
            constrained.syntax = syntax;
            constrained.constraints =
                QueryConstraintSetV1::unconstrained().with_languages(languages.clone());
            let lowered = lower_native_text_input(&constrained, reserve)
                .expect("language constraint must lower");
            for id in ["markdown", "rust"] {
                assert!(
                    lowered
                        .query
                        .filters
                        .contains(&LqFilter::Lang { id: id.into() }),
                    "{syntax:?} lost lang:{id}"
                );
            }
        }
        let mut conflicting = request("lang:rust needle");
        conflicting.constraints = QueryConstraintSetV1::unconstrained().with_languages(languages);
        assert!(matches!(
            lower_native_text_input(&conflicting, reserve),
            Err(IndexError::InvalidInput(message)) if message.contains("lang:")
        ));
    }
}
