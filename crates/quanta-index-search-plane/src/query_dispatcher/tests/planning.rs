use crate::query_dispatcher::planning::prepare_language_query_v1;
use crate::query_dispatcher::tests::support::common::build_probe_query;

#[test]
fn typed_and_dsl_language_constraints_intersect_before_every_retrieval_lane_v1() {
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{LqFilter, QueryConstraintSetV1};

    let typed = QueryConstraintSetV1::from_languages([
        LanguageCode::new("rust").expect("valid language"),
        LanguageCode::new("python").expect("valid language"),
    ])
    .with_exact_repo_relative_path(
        quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
            .expect("valid exact path"),
    );
    let mut query = build_probe_query("needle");
    query.filters.push(LqFilter::Lang {
        id: "Rust".to_string(),
    });
    let prepared = prepare_language_query_v1(query, &typed).expect("valid constraints");
    assert!(!prepared.force_empty);
    assert!(prepared.query.filters.is_empty());
    assert_eq!(
        prepared
            .constraints
            .language_any_of
            .iter()
            .map(LanguageCode::as_str)
            .collect::<Vec<_>>(),
        vec!["rust"]
    );
    assert_eq!(
        prepared
            .constraints
            .repo_relative_path_exact
            .as_ref()
            .map(quanta_index_contract::ExactRepoRelativePathV1::as_str),
        Some("src/lib.rs"),
        "DSL language composition must preserve the independent path axis"
    );

    let typed_rust =
        QueryConstraintSetV1::from_languages([LanguageCode::new("rust").expect("valid language")]);
    let mut disjoint = build_probe_query("needle");
    disjoint.filters.push(LqFilter::Lang {
        id: "python".to_string(),
    });
    let prepared = prepare_language_query_v1(disjoint, &typed_rust).expect("valid constraints");
    assert!(
        prepared.force_empty,
        "disjoint constraints must not widen to all languages"
    );
    assert!(prepared.constraints.is_unconstrained());
}
