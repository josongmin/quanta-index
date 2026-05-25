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

    #[error("channel: {0}")]
    Channel(#[from] quanta_index_channel::ChannelError),

    #[error("remote {code}: {message}")]
    Remote { code: String, message: String },
}
