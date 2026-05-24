//! Report rendering — `JUnit` XML emitter.
//!
//! XML is hand-rolled (no serde-for-XML) so the wire shape is fully
//! auditable and the writer takes a `dyn Write` for in-memory tests.

pub mod junit;

pub use junit::render_junit;
