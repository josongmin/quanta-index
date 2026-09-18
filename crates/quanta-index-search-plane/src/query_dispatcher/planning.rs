//! Query planning: DSL/typed language-constraint composition, projection
//! selection, and the one executable lexical plan shared by search and explain.

use std::collections::BTreeSet;

use quanta_index_contract::{
    GenerationPin, LqFilter, LqQuery, QueryConstraintIntersectionV1, QueryConstraintSetV1,
    SearchPlaneTrackKind, TextQueryRequest,
};
use quanta_index_core::{
    CoreError, LexicalPolicy, QueryRouteV1, ReadDomainV1, RequiredDomainsV1,
    declare_required_domains_v1,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::read_view::ReadViewRequestV1;
use crate::query_dispatcher::rev_at_time::{
    PreparedLexicalTextQuery, rebind_lexical_query_at_time, rev_at_time_selection,
};
use crate::query_dispatcher::selection::resolve_lexical_request_pin;

impl SearchPlaneDispatcher {
    /// Lower a text request into the one executable lexical plan: lowered
    /// query, composed constraints, the generation it runs against and the
    /// domains it declares.
    ///
    /// The ranked search and the per-candidate explanation (QI-BB-022) both
    /// plan here, so an explanation scores a candidate through exactly the
    /// plan that ranked it. `route` names which of the two is planning: the
    /// declaration is a function of it.
    ///
    /// A `rev:at.time(...)` selector is resolved here, before the execution
    /// view is acquired: it is a generation *selection* (plan §5.6, the
    /// resolution order explicit -> pin -> active), walked through a view
    /// that pins only the history authority of the requested generation and
    /// rebinds the plan to the selected commit's active generation. The
    /// executed plan carries no `rev:` filter, so the execution view it
    /// declares does not require history.
    pub(super) fn plan_lexical_text_query(
        &self,
        request: &TextQueryRequest,
        route: QueryRouteV1,
    ) -> Result<PlannedLexicalTextQuery, CoreError> {
        let lowered = lower_lexical_text_query(request)?;
        let prepared_language = prepare_language_query_v1(lowered, &request.constraints)?;
        let base_pin = resolve_lexical_request_pin(
            self.activation_catalog.as_ref(),
            request,
            SearchPlaneTrackKind::Lexical,
            "lexical",
        )?;
        let PreparedLanguageQueryV1 {
            query,
            constraints,
            force_empty: language_force_empty,
        } = prepared_language;
        let prepared = match rev_at_time_selection(&query)? {
            Some(selection) => {
                let selection_view = self.acquire_read_view(&ReadViewRequestV1::selection(
                    "lexical",
                    ReadDomainV1::History,
                    &base_pin,
                ))?;
                rebind_lexical_query_at_time(
                    self.activation_catalog.as_ref(),
                    &selection_view.history()?.state,
                    &base_pin,
                    query,
                    &selection,
                )?
            }
            None => PreparedLexicalTextQuery {
                pin: base_pin,
                query,
                force_empty: false,
            },
        };
        LexicalPolicy::validate_query_with_constraints(&prepared.query, &constraints)?;
        let domains = declare_required_domains_v1(route, Some(&prepared.query));
        Ok(PlannedLexicalTextQuery {
            pin: prepared.pin,
            query: prepared.query,
            constraints,
            force_empty: prepared.force_empty || language_force_empty,
            domains,
        })
    }
}

pub(super) struct PreparedLanguageQueryV1 {
    pub(super) query: LqQuery,
    pub(super) constraints: QueryConstraintSetV1,
    pub(super) force_empty: bool,
}

/// Compose DSL `lang:` filters with the typed OR-set once.
///
/// Remove the DSL language leaves so every sparse and dense lane consumes the
/// same canonical constraint. The two surfaces intersect; a disjoint
/// intersection is an explicit empty result, never an unconstrained fallback.
pub(super) fn prepare_language_query_v1(
    mut query: LqQuery,
    typed: &QueryConstraintSetV1,
) -> Result<PreparedLanguageQueryV1, CoreError> {
    let mut dsl_languages = BTreeSet::new();
    let mut retained = Vec::with_capacity(query.filters.len());
    for filter in std::mem::take(&mut query.filters) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "new non-language filters must remain executable; only lang filters are consumed into the typed constraint set"
        )]
        match filter {
            LqFilter::Lang { id } => {
                let canonical = id.trim().to_ascii_lowercase();
                let language =
                    quanta_index_contract::lex::LanguageCode::new(canonical).map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "query language constraint is not canonical: {err}"
                        ))
                    })?;
                let _inserted = dsl_languages.insert(language);
            }
            other => retained.push(other),
        }
    }
    query.filters = retained;
    let dsl = QueryConstraintSetV1::from_languages(dsl_languages);
    let (constraints, force_empty) = match typed.intersect(&dsl) {
        QueryConstraintIntersectionV1::Compatible(constraints) => (constraints, false),
        QueryConstraintIntersectionV1::Contradiction => {
            (QueryConstraintSetV1::unconstrained(), true)
        }
    };
    Ok(PreparedLanguageQueryV1 {
        query,
        constraints,
        force_empty,
    })
}

pub(super) fn query_selects_file_owner_projection(query: &LqQuery) -> bool {
    query.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::Select {
                dim: quanta_index_contract::LqSelect::FileOwners
            }
        )
    })
}

/// The one executable lexical plan for a text request.
pub(super) struct PlannedLexicalTextQuery {
    pub(super) pin: GenerationPin,
    pub(super) query: LqQuery,
    pub(super) constraints: QueryConstraintSetV1,
    pub(super) force_empty: bool,
    /// The domains the plan reads, declared from the executed query.
    pub(super) domains: RequiredDomainsV1,
}
