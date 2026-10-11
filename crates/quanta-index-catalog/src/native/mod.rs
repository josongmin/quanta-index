//! Native incremental catalog authority on the existing `SQLite` connection.

mod authority;

pub(crate) use authority::initialize;
