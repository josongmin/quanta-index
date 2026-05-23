#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LqDirectiveSet {
    pub directives: Vec<LqDirective>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LqDirective {
    IntoCodeQl,
    ScopeResults,
    WithLexical,
    Custom(String),
}
