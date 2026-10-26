//! Kernel-authenticated foreground telemetry over the bounded control route.

use std::{fs::File, io::Read, sync::Arc, time::Duration};

use quanta_index_contract::native_protocol::{
    NativeIndexRequest, NativeIndexResponse, NativeInteractionPublisherId,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, WorkBudget, effort::ReservationGuard},
};
use quanta_index_ipc::DispatchContextV1;

use crate::native::NativeDispatcher;

pub(super) fn dispatch(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    request: &NativeIndexRequest,
    budget: &RequestBudgetV1,
) -> Result<NativeIndexResponse, IndexError> {
    WorkBudget::with_request(dispatcher.limits, budget.clone()).checkpoint()?;
    let peer = context.principal.ok_or_else(|| {
        IndexError::InvalidInput("foreground publication has no authenticated principal".into())
    })?;
    let recorder = dispatcher
        .interaction
        .get()
        .filter(|_recorder| dispatcher.effort.get().is_some() || dispatcher.overlay.get().is_some())
        .ok_or_else(|| IndexError::InvalidInput("foreground publisher is not configured".into()))?;
    let _reservation = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.control_scheduler),
        dispatcher.control_need,
    )
    .ok_or_else(|| dispatcher.control_scheduler.denial())?;
    match request {
        NativeIndexRequest::InteractionBegin { window_millis } => {
            if !(1..=60_000).contains(window_millis) {
                return Err(IndexError::InvalidInput(
                    "foreground publisher window must be 1..=60000 ms".into(),
                ));
            }
            let mut token = [0_u8; 32];
            File::open("/dev/urandom")
                .and_then(|mut source| source.read_exact(&mut token))
                .map_err(|error| {
                    IndexError::Storage(format!(
                        "foreground publisher entropy unavailable: {error}"
                    ))
                })?;
            let publisher = NativeInteractionPublisherId(token);
            recorder.begin(peer.uid, publisher, Duration::from_millis(*window_millis))?;
            Ok(NativeIndexResponse::InteractionBegun(publisher))
        }
        NativeIndexRequest::InteractionActivity {
            publisher,
            sequence,
        } => {
            recorder.activity(peer.uid, *publisher, *sequence)?;
            Ok(NativeIndexResponse::InteractionAccepted(*publisher))
        }
        NativeIndexRequest::InteractionRenew {
            publisher,
            sequence,
        } => {
            recorder.renew(peer.uid, *publisher, *sequence)?;
            Ok(NativeIndexResponse::InteractionAccepted(*publisher))
        }
        NativeIndexRequest::InteractionEnd {
            publisher,
            sequence,
        } => {
            recorder.end(peer.uid, *publisher, *sequence)?;
            Ok(NativeIndexResponse::InteractionAccepted(*publisher))
        }
        NativeIndexRequest::Overlay(_)
        | NativeIndexRequest::Stub(_)
        | NativeIndexRequest::BindOverlaySave(_)
        | NativeIndexRequest::CompleteOverlaySave(_)
        | NativeIndexRequest::QueryGraph { .. }
        | NativeIndexRequest::QueryRepoMap { .. }
        | NativeIndexRequest::LookupSourceOwners { .. }
        | NativeIndexRequest::ListProducerSourceOwners { .. }
        | NativeIndexRequest::DiffProjectionSourceOwners { .. }
        | NativeIndexRequest::CreateWorkspace { .. }
        | NativeIndexRequest::BeginInput(_)
        | NativeIndexRequest::StageSource { .. }
        | NativeIndexRequest::StageSources { .. }
        | NativeIndexRequest::StageProfile { .. }
        | NativeIndexRequest::StageProjectionOutput { .. }
        | NativeIndexRequest::StageProjectionOwner { .. }
        | NativeIndexRequest::StageProjectionCertificate { .. }
        | NativeIndexRequest::StageMutationPage { .. }
        | NativeIndexRequest::SealMutations { .. }
        | NativeIndexRequest::DiscardInput(_)
        | NativeIndexRequest::SearchLiteral { .. }
        | NativeIndexRequest::QueryPaths { .. }
        | NativeIndexRequest::SearchOverlayText(_)
        | NativeIndexRequest::ReadSession(_)
        | NativeIndexRequest::SearchText { .. }
        | NativeIndexRequest::SearchSemantic { .. }
        | NativeIndexRequest::QueryEffortStream { .. }
        | NativeIndexRequest::Submit(_)
        | NativeIndexRequest::Resume(_)
        | NativeIndexRequest::ReadView(_)
        | NativeIndexRequest::ReadProjection { .. }
        | NativeIndexRequest::LookupProjectionOwners { .. }
        | NativeIndexRequest::Status(_)
        | NativeIndexRequest::ReadyWork { .. }
        | NativeIndexRequest::ClaimWork { .. }
        | NativeIndexRequest::RenewWork { .. }
        | NativeIndexRequest::WorkStatus(_) => Err(IndexError::InvalidInput(
            "request is not foreground telemetry".into(),
        )),
    }
}
