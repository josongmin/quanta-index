//! Admitted construction of the SAME SDK payload, with caller-owned DATA.
//!
//! The receiving owner binds `Shared` to its existing canonical shared handle
//! and calls that owner's typed into-slot producer. No allocator, counter,
//! source authority, callback, or funding bank lives inside the SDK payload.
//! These APIs admit client construction only; SDK RPC execution has its own
//! serialization, transport and resource obligations.

use std::collections::TryReserveError;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use quanta_index_ipc::IpcError;

use crate::config::{
    RootSourceV1, SocketKindV1, SocketSourceV1, fill_socket_path_v1, socket_path_capacity_v1,
    socket_source_v1,
};
use crate::{ClientProfile, ConnectOptions, QuantaIndexClientPayloadV1};

/// Borrowed authentic construction policy.
///
/// Implementations retain actual
/// funding outside payloads and shared handles, through physical header drop.
/// `Funding` must contain no Source/control/input reference or callback.
pub trait NativeSdkConnectAdmissionV1 {
    type OriginalError;
    type Funding;
    /// Construction retains this opaque handle without reading its payload.
    type Shared;

    fn consume_connect_work_v1(&mut self, units: u64) -> Result<(), Self::OriginalError>;

    /// Admit the SDK's actual `PathBuf` reserve before invoking birth exactly
    /// once. Preserve the returned native success. Keep real funding in the
    /// external bank even if admission refuses after physical birth.
    fn admit_path_birth_v1(
        &mut self,
        bytes: usize,
        funding: &mut Option<Self::Funding>,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::OriginalError>;

    /// Call the existing canonical typed shared owner directly on these slots.
    /// Semantica supplies its genuine funded Core handle and typed into-slot
    /// producer. Payload reads belong to that owner's admitted read loan.
    /// No ordinary Arc/adopter, erased owner, or newly implemented reference
    /// counter.
    ///
    /// An error, including after header birth, leaves every candidate in the
    /// supplied external slots and retains actual header funding in the bank.
    fn birth_client_into_slots_v1(
        &mut self,
        payload: &mut Option<QuantaIndexClientPayloadV1>,
        shared: &mut Option<Self::Shared>,
        funding: &mut Option<Self::Funding>,
    ) -> Result<(), Self::OriginalError>;
}

/// Finite attempt/transfer status. Complete causes remain in external DATA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSdkConnectRefusalV1 {
    UsedData,
    OperationRefused,
    OccupiedOutput,
    MissingResult,
}

/// Complete original admission and I/O-policy causes.
///
/// Neither cause is formatted or cloned inside construction. A physical reserve cause has its separate
/// external slot, so even a later admission refusal cannot discard it.
#[derive(Debug)]
pub enum NativeSdkConnectFailureV1<E> {
    Admission(E),
    IoPolicy(IpcError),
    SocketUnresolved(&'static str),
    UnsupportedEnvironmentLookup,
    PathAllocation,
    ArithmeticOverflow,
    InvalidNativeProducer,
    InvalidNativeCapacity,
}

/// Allocation-free storage for one construction attempt.
///
/// Physical state drops
/// FIRST; complete errors next; the actual funding bank LAST. Retain the entire
/// DATA through the highest Source finisher. After a pure output transfer the
/// bank must still outlive every strong/weak handle and dependent backing.
/// DATA and a successful producer status do not issue Source authority.
pub struct NativeSdkConnectDataV1<E, F, H> {
    shared: Option<H>,
    payload: Option<QuantaIndexClientPayloadV1>,
    paths: [PathBuf; 3],
    reserve_failure: Option<TryReserveError>,
    failure: Option<NativeSdkConnectFailureV1<E>>,
    attempted: bool,
    completed: bool,
    funding: Option<F>,
}

impl<E, F, H> NativeSdkConnectDataV1<E, F, H> {
    #[must_use]
    pub const fn new_v1() -> Self {
        Self {
            shared: None,
            payload: None,
            paths: [PathBuf::new(), PathBuf::new(), PathBuf::new()],
            reserve_failure: None,
            failure: None,
            attempted: false,
            completed: false,
            funding: None,
        }
    }

    #[must_use]
    pub fn failure_v1(&self) -> Option<&NativeSdkConnectFailureV1<E>> {
        self.failure.as_ref()
    }

    #[must_use]
    pub fn reserve_failure_v1(&self) -> Option<&TryReserveError> {
        self.reserve_failure.as_ref()
    }

    /// Producer completion only. This neither reads the opaque handle nor
    /// replaces the receiving Source's terminal classification.
    #[must_use]
    pub fn is_complete_v1(&self) -> bool {
        self.attempted
            && self.completed
            && self.shared.is_some()
            && self.payload.is_none()
            && self.failure.is_none()
            && self.reserve_failure.is_none()
    }

    /// Borrow only the completed opaque handle. No retain, payload read,
    /// funding transfer, or Source authority is issued by this observation.
    #[must_use]
    pub fn complete_shared_v1(&self) -> Option<&H> {
        if self.is_complete_v1() {
            self.shared.as_ref()
        } else {
            None
        }
    }

    /// Pure transfer; occupied outputs are unchanged and never poll admission.
    pub fn complete_into_slot_v1(
        &mut self,
        output: &mut Option<H>,
    ) -> Result<(), NativeSdkConnectRefusalV1> {
        if output.is_some() {
            return Err(NativeSdkConnectRefusalV1::OccupiedOutput);
        }
        if !self.is_complete_v1() {
            return Err(NativeSdkConnectRefusalV1::MissingResult);
        }
        *output = self.shared.take();
        self.completed = false;
        Ok(())
    }
}

impl<E, F, H> Default for NativeSdkConnectDataV1<E, F, H> {
    fn default() -> Self {
        Self::new_v1()
    }
}

impl<E, F, H: Deref<Target = QuantaIndexClientPayloadV1>> NativeSdkConnectDataV1<E, F, H> {
    /// Borrow only a producer-complete client. This does not replace the
    /// receiving Source's terminal classification or admit RPC allocations.
    #[must_use]
    pub fn client_v1(&self) -> Option<&QuantaIndexClientPayloadV1> {
        self.complete_shared_v1().map(Deref::deref)
    }
}

fn reserve_path_v1<P: NativeSdkConnectAdmissionV1 + ?Sized>(
    bytes: usize,
    path: &mut PathBuf,
    reserve_failure: &mut Option<TryReserveError>,
    funding: &mut Option<P::Funding>,
    admission: &mut P,
) -> Result<(), NativeSdkConnectFailureV1<P::OriginalError>> {
    if bytes == 0 {
        return Ok(());
    }
    let mut invoked = false;
    let mut repeated = false;
    let mut succeeded = false;
    let admitted = admission
        .admit_path_birth_v1(bytes, funding, &mut || {
            if invoked {
                repeated = true;
                // Keep the FIRST physical commit receipt even though the
                // repeated callback makes the overall protocol invalid.
                return succeeded;
            }
            invoked = true;
            match path.try_reserve_exact(bytes) {
                Ok(()) => {
                    succeeded = true;
                    true
                }
                Err(cause) => {
                    *reserve_failure = Some(cause);
                    false
                }
            }
        })
        .map_err(NativeSdkConnectFailureV1::Admission)?;
    if !invoked || repeated || admitted != succeeded {
        return Err(NativeSdkConnectFailureV1::InvalidNativeProducer);
    }
    if !succeeded {
        return Err(NativeSdkConnectFailureV1::PathAllocation);
    }
    if path.capacity() != bytes {
        return Err(NativeSdkConnectFailureV1::InvalidNativeCapacity);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SocketInputV1<'input> {
    root: Option<&'input Path>,
    explicit: Option<&'input Path>,
    kind: SocketKindV1,
}

fn construct_path_v1<P: NativeSdkConnectAdmissionV1 + ?Sized>(
    input: SocketInputV1<'_>,
    path: &mut PathBuf,
    reserve_failure: &mut Option<TryReserveError>,
    funding: &mut Option<P::Funding>,
    admission: &mut P,
) -> Result<(), NativeSdkConnectFailureV1<P::OriginalError>> {
    let SocketInputV1 {
        root,
        explicit,
        kind,
    } = input;
    admission
        .consume_connect_work_v1(1)
        .map_err(NativeSdkConnectFailureV1::Admission)?;
    let source = socket_source_v1(root.is_some(), explicit.is_some()).ok_or(
        NativeSdkConnectFailureV1::SocketUnresolved(kind.unresolved_v1()),
    )?;
    let input = match source {
        SocketSourceV1::Explicit => explicit,
        SocketSourceV1::StateRoot => root,
    }
    .ok_or(NativeSdkConnectFailureV1::InvalidNativeProducer)?;
    let bytes = socket_path_capacity_v1(input, source, kind)
        .ok_or(NativeSdkConnectFailureV1::ArithmeticOverflow)?;
    let work =
        u64::try_from(bytes).map_err(|_overflow| NativeSdkConnectFailureV1::ArithmeticOverflow)?;
    admission
        .consume_connect_work_v1(work)
        .map_err(NativeSdkConnectFailureV1::Admission)?;
    reserve_path_v1(bytes, path, reserve_failure, funding, admission)?;
    // A late birth refusal already returned with physical backing parked.
    // On success the pre-reserved canonical fill performs no new allocation.
    fill_socket_path_v1(input, source, kind, path);
    if path.capacity() != bytes {
        return Err(NativeSdkConnectFailureV1::InvalidNativeCapacity);
    }
    admission
        .consume_connect_work_v1(0)
        .map_err(NativeSdkConnectFailureV1::Admission)
}

/// Construct the SAME payload before shared birth.
///
/// Every partial and full cause stays external on all outcomes. Options only borrow input during
/// this call; no input/control reference or callback is retained in DATA.
/// Reentry rejects before any policy/deadline/input poll. Native options use
/// explicit roots or sockets; unsupported environment births fail closed.
pub fn try_connect_native_into_v1<P: NativeSdkConnectAdmissionV1 + ?Sized>(
    options: ConnectOptions<&Path>,
    profile: ClientProfile,
    admission: &mut P,
    data: &mut NativeSdkConnectDataV1<P::OriginalError, P::Funding, P::Shared>,
) -> Result<(), NativeSdkConnectRefusalV1> {
    if data.attempted {
        return Err(NativeSdkConnectRefusalV1::UsedData);
    }
    data.attempted = true;
    let result = (|| {
        admission
            .consume_connect_work_v1(0)
            .map_err(NativeSdkConnectFailureV1::Admission)?;
        let io_policy = options
            .io_policy_v1()
            .map_err(NativeSdkConnectFailureV1::IoPolicy)?;
        admission
            .consume_connect_work_v1(1)
            .map_err(NativeSdkConnectFailureV1::Admission)?;
        let root = match options.root_source_v1() {
            RootSourceV1::Explicit => options.state_root,
            RootSourceV1::PinnedSockets => None,
            RootSourceV1::Environment => {
                return Err(NativeSdkConnectFailureV1::UnsupportedEnvironmentLookup);
            }
        };
        construct_path_v1(
            SocketInputV1 {
                root,
                explicit: options.query_socket,
                kind: SocketKindV1::Query,
            },
            &mut data.paths[0],
            &mut data.reserve_failure,
            &mut data.funding,
            admission,
        )?;
        if profile == ClientProfile::Full {
            construct_path_v1(
                SocketInputV1 {
                    root,
                    explicit: options.control_socket,
                    kind: SocketKindV1::Control,
                },
                &mut data.paths[1],
                &mut data.reserve_failure,
                &mut data.funding,
                admission,
            )?;
            construct_path_v1(
                SocketInputV1 {
                    root,
                    explicit: options.ingest_socket,
                    kind: SocketKindV1::Ingest,
                },
                &mut data.paths[2],
                &mut data.reserve_failure,
                &mut data.funding,
                admission,
            )?;
        }
        admission
            .consume_connect_work_v1(1)
            .map_err(NativeSdkConnectFailureV1::Admission)?;
        let (control, ingest) = if profile == ClientProfile::Full {
            (
                Some(core::mem::take(&mut data.paths[1])),
                Some(core::mem::take(&mut data.paths[2])),
            )
        } else {
            (None, None)
        };
        data.payload = Some(QuantaIndexClientPayloadV1::from_socket_paths_v1(
            core::mem::take(&mut data.paths[0]),
            control,
            ingest,
            io_policy,
        ));
        admission
            .birth_client_into_slots_v1(&mut data.payload, &mut data.shared, &mut data.funding)
            .map_err(NativeSdkConnectFailureV1::Admission)?;
        if data.payload.is_some() || data.shared.is_none() {
            return Err(NativeSdkConnectFailureV1::InvalidNativeProducer);
        }
        admission
            .consume_connect_work_v1(0)
            .map_err(NativeSdkConnectFailureV1::Admission)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            data.completed = true;
            Ok(())
        }
        Err(cause) => {
            data.failure = Some(cause);
            Err(NativeSdkConnectRefusalV1::OperationRefused)
        }
    }
}

#[cfg(test)]
mod tests;
