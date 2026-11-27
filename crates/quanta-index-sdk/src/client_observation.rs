//! Request-local query RPC observations shared by SDK transports.

use quanta_index_ipc::ClientIpcTimingV1;

/// The query-plane RPC made during one observed lexical SDK call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientQueryRpcKindV1 {
    ResolveActiveGeneration,
    ResolveLexicalGeneration,
    Text,
    NativeResolveProtectedSourceHead,
    NativeOpenReadSession,
    NativeReadSessionProfile,
    NativeTextPathPage,
}

impl ClientQueryRpcKindV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResolveActiveGeneration => "query.resolve_active",
            Self::ResolveLexicalGeneration => "query.resolve_lexical",
            Self::Text => "query.text",
            Self::NativeResolveProtectedSourceHead => "native.resolve_protected_source_head",
            Self::NativeOpenReadSession => "native.read_session.open",
            Self::NativeReadSessionProfile => "native.read_session.profile",
            Self::NativeTextPathPage => "native.read_session.text_path_page",
        }
    }
}

/// Request-ID-bound transport intervals for one successful query-plane RPC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientQueryRpcObservationV1 {
    pub kind: ClientQueryRpcKindV1,
    pub request_id: u64,
    pub ipc: ClientIpcTimingV1,
}

/// Successful foreground RPCs in the execution order of one lexical SDK query.
/// Native lease renewal and cleanup are outside this observation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClientLexicalQueryObservationV1 {
    pub rpcs: Vec<ClientQueryRpcObservationV1>,
}
