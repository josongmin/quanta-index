pub mod domain_engine;
#[cfg(test)]
mod stub_engine;

pub use domain_engine::{DomainQueryEngine, QueryEmbedder};
#[cfg(test)]
pub use stub_engine::StubQueryEngine;
