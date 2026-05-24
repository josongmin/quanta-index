//! Abstract channel surface. Callers depend on these traits and on the factory
//! functions exported from the crate root — never on transport-implementation
//! modules. Cross-module visibility is enforced by the hexagonal boundary lint.

pub mod error;
pub mod publisher;
pub mod subscriber;
