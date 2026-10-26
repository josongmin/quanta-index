//! Borrowed `RepoMap` publication over the existing canonical ingest/control wire.

use ciborium::de::FiniteParserAllocationAdmissionV3;
use quanta_index_contract::{
    BorrowedAuxiliaryIngestIpcRequestV1, BorrowedRepoMapPublishBundleRequestV3,
    RepoMapActivateGenerationRequestV2, RepoMapSourceBundleDigestWorkErrorV3,
    RepoMapTerminalReceiptV2, SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope,
    SourceIdentityDecodeAdmissionV1, admitted_auxiliary_response_seed_v1,
    admitted_repo_map_control_response_seed_v3,
    canonical_repo_map_source_bundle_digest_matches_v2_with_work_v3,
};
use quanta_index_ipc::{
    AdmittedCborDecodeErrorV1, AdmittedClientResponseBody, AdmittedDecodedCborV1, IpcError,
    MemoryScope, RequestBudgetV1, decode_cbor_seed_admitted_v1,
};

use super::RepoMapNamespace;
use crate::SdkError;

/// The decoded receipt and its admitted response backing travel together.
pub struct AdmittedRepoMapTerminalReceiptV3<O> {
    inner: AdmittedRepoMapTerminalReceiptInnerV3<O>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "decoded envelope, parser owner, and admitted raw response stay inline under the original memory grant"
)]
enum AdmittedRepoMapTerminalReceiptInnerV3<O> {
    Publish {
        decoded: AdmittedDecodedCborV1<SearchPlaneIngestIpcResponseEnvelope, O>,
        _body: AdmittedClientResponseBody,
    },
    Activate {
        decoded: AdmittedDecodedCborV1<SearchPlaneControlIpcResponseEnvelope, O>,
        _body: AdmittedClientResponseBody,
    },
}

impl<O> AdmittedRepoMapTerminalReceiptV3<O> {
    #[must_use]
    pub fn receipt(&self) -> Option<&RepoMapTerminalReceiptV2> {
        match &self.inner {
            AdmittedRepoMapTerminalReceiptInnerV3::Publish { decoded, .. } => {
                if let SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt) =
                    &decoded.value().payload
                {
                    Some(receipt)
                } else {
                    None
                }
            }
            AdmittedRepoMapTerminalReceiptInnerV3::Activate { decoded, .. } => {
                if let SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(receipt) =
                    &decoded.value().payload
                {
                    Some(receipt)
                } else {
                    None
                }
            }
        }
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "remote errors retain the decoded envelope, parser owner, and admitted raw response without a new allocation"
)]
pub enum RepoMapAdmittedPublishErrorV3<E, O> {
    /// Digest work was refused before any transport attempt began.
    BeforeSendProgress(E),
    /// The transport or admitted response failed after an attempt may have begun.
    Progress(E),
    DigestEncode(quanta_index_contract::RepoMapSourceBundleDigestEncodeErrorV3),
    DigestMismatch,
    Sdk(SdkError),
    Decode(AdmittedCborDecodeErrorV1<E>),
    ResponseRequestIdMismatch,
    ResponseVariantMismatch,
    RemotePublish {
        decoded: AdmittedDecodedCborV1<SearchPlaneIngestIpcResponseEnvelope, O>,
        _body: AdmittedClientResponseBody,
    },
    RemoteActivate {
        decoded: AdmittedDecodedCborV1<SearchPlaneControlIpcResponseEnvelope, O>,
        _body: AdmittedClientResponseBody,
    },
}

impl<E: std::fmt::Debug, O> std::fmt::Debug for RepoMapAdmittedPublishErrorV3<E, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeSendProgress(cause) => {
                f.debug_tuple("BeforeSendProgress").field(cause).finish()
            }
            Self::Progress(cause) => f.debug_tuple("Progress").field(cause).finish(),
            Self::DigestEncode(cause) => f.debug_tuple("DigestEncode").field(cause).finish(),
            Self::DigestMismatch => f.write_str("DigestMismatch"),
            Self::Sdk(cause) => f.debug_tuple("Sdk").field(cause).finish(),
            Self::Decode(cause) => f.debug_tuple("Decode").field(cause).finish(),
            Self::ResponseRequestIdMismatch => f.write_str("ResponseRequestIdMismatch"),
            Self::ResponseVariantMismatch => f.write_str("ResponseVariantMismatch"),
            Self::RemotePublish { .. } => f.write_str("RemotePublish"),
            Self::RemoteActivate { .. } => f.write_str("RemoteActivate"),
        }
    }
}

impl<E: std::fmt::Display, O> std::fmt::Display for RepoMapAdmittedPublishErrorV3<E, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeSendProgress(cause) => {
                write!(f, "RepoMap digest work refused before send: {cause}")
            }
            Self::Progress(cause) => write!(f, "RepoMap publication interrupted: {cause}"),
            Self::DigestEncode(cause) => write!(f, "RepoMap digest encode: {cause}"),
            Self::DigestMismatch => f.write_str("RepoMap source bundle digest mismatch"),
            Self::Sdk(cause) => write!(f, "RepoMap SDK: {cause}"),
            Self::Decode(_) => f.write_str("RepoMap response decode refused"),
            Self::ResponseRequestIdMismatch => f.write_str("RepoMap response request id mismatch"),
            Self::ResponseVariantMismatch => f.write_str("RepoMap response variant mismatch"),
            Self::RemotePublish { decoded, .. } => {
                let SearchPlaneIngestIpcResponse::Error(error) = &decoded.value().payload else {
                    return f.write_str("RepoMap publish remote response variant mismatch");
                };
                write!(
                    f,
                    "RepoMap publish remote {}: {}",
                    error.code, error.message
                )?;
                if let Some(repair) = &error.repair {
                    write!(f, " repair={repair:?}")?;
                }
                Ok(())
            }
            Self::RemoteActivate { decoded, .. } => {
                let SearchPlaneControlIpcResponse::Error(error) = &decoded.value().payload else {
                    return f.write_str("RepoMap activation remote response variant mismatch");
                };
                write!(
                    f,
                    "RepoMap activation remote {}: {}",
                    error.code, error.message
                )?;
                if let Some(repair) = &error.repair {
                    write!(f, " repair={repair:?}")?;
                }
                Ok(())
            }
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display, O> std::error::Error
    for RepoMapAdmittedPublishErrorV3<E, O>
{
}

enum SendWithWorkErrorV3<E> {
    Progress(E),
    Sdk(SdkError),
}

fn send_with_work_v3<E>(
    work: &mut impl FnMut(usize) -> Result<(), E>,
    send: impl FnOnce(
        &mut dyn FnMut(usize) -> Result<(), IpcError>,
    ) -> Result<(u64, AdmittedClientResponseBody), SdkError>,
) -> Result<(u64, AdmittedClientResponseBody), SendWithWorkErrorV3<E>> {
    let mut refused = None;
    let send_result = send(&mut |units| match work(units) {
        Ok(()) => Ok(()),
        Err(cause) => {
            refused = Some(cause);
            Err(IpcError::Io(std::io::ErrorKind::Interrupted.into()))
        }
    });
    refused.map_or_else(
        || send_result.map_err(SendWithWorkErrorV3::Sdk),
        |cause| Err(SendWithWorkErrorV3::Progress(cause)),
    )
}

impl RepoMapNamespace<'_> {
    /// Send the canonical publish wire without copying the retained source bundle.
    #[expect(
        clippy::result_large_err,
        reason = "the remote error retains its decoded envelope, parser owner, and admitted raw response inline"
    )]
    pub fn publish_borrowed_admitted_v3<E, F, O>(
        &self,
        request: BorrowedRepoMapPublishBundleRequestV3<'_>,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
        owner: O,
        work: &mut F,
    ) -> Result<AdmittedRepoMapTerminalReceiptV3<O>, RepoMapAdmittedPublishErrorV3<E, O>>
    where
        E: std::fmt::Debug,
        F: FnMut(usize) -> Result<(), E>,
        O: Clone + SourceIdentityDecodeAdmissionV1 + FiniteParserAllocationAdmissionV3,
    {
        match canonical_repo_map_source_bundle_digest_matches_v2_with_work_v3(
            request.bundle,
            request.source_bundle_digest,
            work,
        ) {
            Ok(true) => {}
            Ok(false) => return Err(RepoMapAdmittedPublishErrorV3::DigestMismatch),
            Err(RepoMapSourceBundleDigestWorkErrorV3::Work(cause)) => {
                return Err(RepoMapAdmittedPublishErrorV3::BeforeSendProgress(cause));
            }
            Err(RepoMapSourceBundleDigestWorkErrorV3::Encode(cause)) => {
                return Err(RepoMapAdmittedPublishErrorV3::DigestEncode(cause));
            }
        }
        let (request_id, body) = send_with_work_v3(work, |send_work| {
            self.client.dispatch_borrowed_auxiliary_ingest_admitted_v1(
                BorrowedAuxiliaryIngestIpcRequestV1::RepoMap(request),
                budget,
                memory,
                send_work,
            )
        })
        .map_err(map_send_error_v3)?;
        let visitor_owner = owner.clone();
        let mut scratch = [0_u8; 128];
        let decoded = decode_cbor_seed_admitted_v1(
            body.as_slice(),
            &mut scratch,
            admitted_auxiliary_response_seed_v1(&visitor_owner, body.as_slice().len()),
            work,
            quanta_index_contract::MAX_IPC_FRAME_BODY_BYTES_V1,
            owner,
        )
        .map_err(map_decode_error_v3)?;
        if decoded.value().request_id != request_id {
            return Err(RepoMapAdmittedPublishErrorV3::ResponseRequestIdMismatch);
        }
        if let SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt) =
            &decoded.value().payload
        {
            crate::binding::check_repo_map_publish_receipt_v3(receipt, request)
                .map_err(RepoMapAdmittedPublishErrorV3::Sdk)?;
        } else if let SearchPlaneIngestIpcResponse::Error(_) = &decoded.value().payload {
            return Err(RepoMapAdmittedPublishErrorV3::RemotePublish {
                decoded,
                _body: body,
            });
        } else {
            return Err(RepoMapAdmittedPublishErrorV3::ResponseVariantMismatch);
        }
        Ok(AdmittedRepoMapTerminalReceiptV3 {
            inner: AdmittedRepoMapTerminalReceiptInnerV3::Publish {
                decoded,
                _body: body,
            },
        })
    }

    /// Activate the same candidate through the canonical borrowed control wire.
    #[expect(
        clippy::result_large_err,
        reason = "the remote error retains its decoded envelope, parser owner, and admitted raw response inline"
    )]
    pub fn activate_borrowed_admitted_v3<E, F, O>(
        &self,
        request: &RepoMapActivateGenerationRequestV2,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
        owner: O,
        work: &mut F,
    ) -> Result<AdmittedRepoMapTerminalReceiptV3<O>, RepoMapAdmittedPublishErrorV3<E, O>>
    where
        E: std::fmt::Debug,
        F: FnMut(usize) -> Result<(), E>,
        O: Clone + SourceIdentityDecodeAdmissionV1 + FiniteParserAllocationAdmissionV3,
    {
        let (request_id, body) = send_with_work_v3(work, |send_work| {
            self.client
                .dispatch_borrowed_repo_map_activate_admitted_v3(request, budget, memory, send_work)
        })
        .map_err(map_send_error_v3)?;
        let visitor_owner = owner.clone();
        let mut scratch = [0_u8; 128];
        let decoded = decode_cbor_seed_admitted_v1(
            body.as_slice(),
            &mut scratch,
            admitted_repo_map_control_response_seed_v3(&visitor_owner, body.as_slice().len()),
            work,
            quanta_index_contract::MAX_IPC_FRAME_BODY_BYTES_V1,
            owner,
        )
        .map_err(map_decode_error_v3)?;
        if decoded.value().request_id != request_id {
            return Err(RepoMapAdmittedPublishErrorV3::ResponseRequestIdMismatch);
        }
        if let SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(receipt) =
            &decoded.value().payload
        {
            crate::binding::check_repo_map_activate_receipt_v3(receipt, request)
                .map_err(RepoMapAdmittedPublishErrorV3::Sdk)?;
        } else if let SearchPlaneControlIpcResponse::Error(_) = &decoded.value().payload {
            return Err(RepoMapAdmittedPublishErrorV3::RemoteActivate {
                decoded,
                _body: body,
            });
        } else {
            return Err(RepoMapAdmittedPublishErrorV3::ResponseVariantMismatch);
        }
        Ok(AdmittedRepoMapTerminalReceiptV3 {
            inner: AdmittedRepoMapTerminalReceiptInnerV3::Activate {
                decoded,
                _body: body,
            },
        })
    }
}

fn map_send_error_v3<E, O>(error: SendWithWorkErrorV3<E>) -> RepoMapAdmittedPublishErrorV3<E, O> {
    match error {
        SendWithWorkErrorV3::Progress(cause) => RepoMapAdmittedPublishErrorV3::Progress(cause),
        SendWithWorkErrorV3::Sdk(cause) => RepoMapAdmittedPublishErrorV3::Sdk(cause),
    }
}

fn map_decode_error_v3<E, O>(
    error: AdmittedCborDecodeErrorV1<E>,
) -> RepoMapAdmittedPublishErrorV3<E, O> {
    if let AdmittedCborDecodeErrorV1::Work(cause) = error {
        RepoMapAdmittedPublishErrorV3::Progress(cause)
    } else {
        RepoMapAdmittedPublishErrorV3::Decode(error)
    }
}
