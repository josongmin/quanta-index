#![forbid(unsafe_code)]

use quanta_index_contract::{
    LQ_VERSION_TAG, LqExpr, LqFilter, LqLeaf, LqOptions, LqPredicateArg, LqQuery, LqSelect, LqSpan,
    LqType,
};
use quanta_index_core::{CoreError, domains::lexical::LexicalPolicy};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn query_with_filters(filters: Vec<LqFilter>) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
        filters,
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

#[test]
fn lexical_policy_allows_repo_select_surface() -> TestResult {
    if let Err(err) = LexicalPolicy::validate_query(&query_with_filters(vec![LqFilter::Select {
        dim: LqSelect::Repo,
    }])) {
        return Err(format!("expected select repo acceptance, got {err:?}").into());
    }
    Ok(())
}

#[test]
fn lexical_policy_rejects_non_lexical_type_surface() -> TestResult {
    match LexicalPolicy::validate_query(&query_with_filters(vec![LqFilter::Type {
        kind: LqType::Commit,
    }])) {
        Err(CoreError::NotImplemented(message)) if message.contains("type filter `commit`") => {
            Ok(())
        }
        other => Err(format!("expected type commit rejection, got {other:?}").into()),
    }
}

#[test]
fn lexical_policy_rejects_rev_surface() -> TestResult {
    match LexicalPolicy::validate_query(&query_with_filters(vec![LqFilter::Rev {
        spec: "main".to_string(),
    }])) {
        Err(CoreError::NotImplemented(message)) if message.contains("rev filter") => Ok(()),
        other => Err(format!("expected rev rejection, got {other:?}").into()),
    }
}

#[test]
fn lexical_policy_allows_predicate_under_or_and_not() -> TestResult {
    let predicate = LqExpr::Leaf(LqLeaf::Predicate {
        name: "repo.has.file".to_string(),
        args: vec![LqPredicateArg::Filter {
            name: "path".to_string(),
            value: "src/lib.rs".to_string(),
        }],
    });
    let or_query = LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Any(vec![
            predicate.clone(),
            LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
        ]),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    };
    if let Err(err) = LexicalPolicy::validate_query(&or_query) {
        return Err(format!("expected OR predicate acceptance, got {err:?}").into());
    }

    let not_query = LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Not(Box::new(predicate)),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    };
    if let Err(err) = LexicalPolicy::validate_query(&not_query) {
        return Err(format!("expected NOT predicate acceptance, got {err:?}").into());
    }
    Ok(())
}
