use quanta_index_contract::{LqExpr, LqQuery};

use crate::CoreError;

pub struct QueryPolicy;

impl QueryPolicy {
    pub fn validate_query(query: &LqQuery) -> Result<(), CoreError> {
        match &query.expr {
            LqExpr::MatchAll => Err(CoreError::InvalidContract(
                "match-all query is not allowed on external search plane".into(),
            )),
            LqExpr::Raw(_) | LqExpr::All(_) | LqExpr::Any(_) | LqExpr::Not(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{LqDirectiveSet, LqExpr, LqFilterSet, LqOptionSet, LqQuery};

    use super::QueryPolicy;
    use crate::CoreError;

    #[test]
    fn rejects_match_all_queries() {
        let result = QueryPolicy::validate_query(&sample_query(LqExpr::MatchAll));
        assert!(
            matches!(result, Err(CoreError::InvalidContract(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn accepts_raw_queries() {
        let result = QueryPolicy::validate_query(&sample_query(LqExpr::Raw("symbol:Foo".into())));
        assert!(result.is_ok(), "unexpected result: {result:?}");
    }

    fn sample_query(expr: LqExpr) -> LqQuery {
        LqQuery {
            expr,
            filters: LqFilterSet::default(),
            options: LqOptionSet::default(),
            directives: LqDirectiveSet::default(),
        }
    }
}
