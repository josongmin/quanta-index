use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("usage: {0}")]
    Usage(String),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("serialization: {0}")]
    Serialization(String),

    #[error("transport: {0}")]
    Transport(#[source] quanta_index_ipc::IpcError),

    // QI-SDK-01: SDK no longer opens channel publishers directly, so the
    // `Channel(ChannelError)` variant was removed alongside the dependency
    // drop. Channel-level failures now surface through the ingest
    // dispatcher as typed `SearchPlaneIpcError` (code/message) and arrive
    // here as `Remote { code, message }`.
    #[error("remote {code}: {message}")]
    Remote { code: String, message: String },
}
