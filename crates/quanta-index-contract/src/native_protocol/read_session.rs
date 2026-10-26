//! Product read leases reference the Catalog-owned ticket retained by the daemon.

use crate::{
    TextQueryRequest,
    index::{IndexInstanceId, ViewRef, ViewSelection},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NativeReadSessionId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReadSessionState {
    Active,
    Closing,
    Released,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSessionStatus {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub expires_at_ms: u64,
    pub revision: u64,
    pub state: NativeReadSessionState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReadSettlementState {
    Pending,
    Incomplete,
    SettledAtSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSessionSettlement {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub head: ViewRef,
    pub exact_floor: u64,
    pub state: NativeReadSettlementState,
    /// Remaining target pins in this settlement snapshot, rather than at release.
    pub has_remaining_target_pins: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeReadSessionRequest {
    Open {
        selection: ViewSelection,
        open_key: [u8; 32],
    },
    Renew {
        session: NativeReadSessionId,
        revision: u64,
    },
    Status(NativeReadSessionId),
    SearchText {
        session: NativeReadSessionId,
        instance: IndexInstanceId,
        query: TextQueryRequest,
    },
    Release(NativeReadSessionId),
    Settlement(NativeReadSessionId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReadSessionReply {
    Status(NativeReadSessionStatus),
    Released {
        status: NativeReadSessionStatus,
        has_remaining_target_pins: bool,
    },
    Settlement(NativeReadSessionSettlement),
}

quanta_index_contract_base::native_serde_newtype!(NativeReadSessionId, [u8; 32]);
quanta_index_contract_base::native_serde_enum!(NativeReadSessionState { units { Active = 0, Closing = 1, Released = 2, Unknown = 3 } newtypes { } records { } });
quanta_index_contract_base::native_serde_enum!(NativeReadSettlementState { units { Pending = 0, Incomplete = 1, SettledAtSnapshot = 2 } newtypes { } records { } });
quanta_index_contract_base::native_serde_struct!(NativeReadSessionStatus {
    session: NativeReadSessionId,
    view: ViewRef,
    expires_at_ms: u64,
    revision: u64,
    state: NativeReadSessionState
});
quanta_index_contract_base::native_serde_struct!(NativeReadSessionSettlement {
    session: NativeReadSessionId,
    view: ViewRef,
    head: ViewRef,
    exact_floor: u64,
    state: NativeReadSettlementState,
    has_remaining_target_pins: bool
});
quanta_index_contract_base::native_serde_enum!(NativeReadSessionRequest { units { } newtypes { Status = 2 => NativeReadSessionId, Release = 4 => NativeReadSessionId, Settlement = 5 => NativeReadSessionId } records { Open = 0 { selection: ViewSelection, open_key: [u8; 32] }, Renew = 1 { session: NativeReadSessionId, revision: u64 }, SearchText = 3 { session: NativeReadSessionId, instance: IndexInstanceId, query: TextQueryRequest } } });
quanta_index_contract_base::native_serde_enum!(NativeReadSessionReply { units { } newtypes { Status = 0 => NativeReadSessionStatus, Settlement = 2 => NativeReadSessionSettlement } records { Released = 1 { status: NativeReadSessionStatus, has_remaining_target_pins: bool } } });

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_protocol::NativeIndexRequest;

    #[test]
    fn read_session_release_has_fixed_public_wire_shape() -> Result<(), Box<dyn std::error::Error>>
    {
        let expected = serde_json::json!({"ReadSession": {"Release": vec![7_u8; 32]}});
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(
            NativeReadSessionId([7; 32]),
        ));
        assert_eq!(serde_json::to_value(&request)?, expected);
        assert_eq!(
            serde_json::from_value::<NativeIndexRequest>(expected)?,
            request
        );
        Ok(())
    }

    #[test]
    fn read_session_handle_decode_refuses_wrong_length_and_unknown_operation() {
        for length in [0, 31, 33] {
            let wire = serde_json::json!({"ReadSession": {"Release": vec![7_u8; length]}});
            assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
        }
        let wire = serde_json::json!({"ReadSession": {"ForceRelease": vec![7_u8; 32]}});
        assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    }
}
