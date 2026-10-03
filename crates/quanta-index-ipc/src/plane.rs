//! Transport plane identity shared by admission and dispatch.

/// Which daemon plane one server serves (S21-10). The transport names it
/// so a dispatch context cannot misreport which socket carried a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpcPlane {
    Query,
    Control,
    Ingest,
}
