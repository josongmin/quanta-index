#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LqOptionSet {
    pub limit: Option<u32>,
    pub count_all: bool,
    pub timeout_ms: Option<u64>,
}
