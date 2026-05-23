#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LqFilterSet {
    pub filters: Vec<LqFilter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LqFilter {
    Repo(String),
    File(String),
    Path(String),
    Lang(String),
    Rev(String),
    Select(String),
    Type(String),
    Custom { key: String, value: String },
}
