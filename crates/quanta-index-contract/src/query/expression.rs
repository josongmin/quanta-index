use crate::{LqDirectiveSet, LqFilterSet, LqOptionSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LqQuery {
    pub expr: LqExpr,
    pub filters: LqFilterSet,
    pub options: LqOptionSet,
    pub directives: LqDirectiveSet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LqExpr {
    MatchAll,
    Raw(String),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}
