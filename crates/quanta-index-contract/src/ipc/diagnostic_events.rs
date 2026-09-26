//! Bounded, operator-only projection of the transport's request-event ring.
//!
//! This is a wire artifact, not a second event store or a request ledger. The
//! control dispatcher authorizes the caller before asking the runtime for a
//! window; the runtime maps its private IPC event IR into these payload-free
//! records. Fixed-length tuples make extra, missing and reordered fields
//! decoding errors on both CBOR and JSON rails.

use core::fmt;
use std::num::NonZeroU64;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::SearchPlaneErrorCodeV2;

pub const MAX_PROCESS_REQUEST_EVENTS_V1: u16 = 1024;
pub const MAX_PROCESS_REQUEST_EVENTS_WIRE_BYTES_V1: usize = 1024 * 1024;

/// Exactly one daemon socket plane, selected by the operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessRequestEventPlaneV1 {
    Query,
    Control,
    Ingest,
}

impl ProcessRequestEventPlaneV1 {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Control => "control",
            Self::Ingest => "ingest",
        }
    }
}

impl Serialize for ProcessRequestEventPlaneV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for ProcessRequestEventPlaneV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "query" => Ok(Self::Query),
            "control" => Ok(Self::Control),
            "ingest" => Ok(Self::Ingest),
            other => Err(de::Error::unknown_variant(
                other,
                &["query", "control", "ingest"],
            )),
        }
    }
}

/// One bounded read. The limit is a wire policy, not the ring's capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessRequestEventsRequestV1 {
    pub plane: ProcessRequestEventPlaneV1,
    pub limit: u16,
}

impl ProcessRequestEventsRequestV1 {
    pub fn validate_v1(&self) -> Result<(), &'static str> {
        if self.limit == 0 || self.limit > MAX_PROCESS_REQUEST_EVENTS_V1 {
            return Err("request-event limit is outside 1..=1024");
        }
        Ok(())
    }
}

impl Serialize for ProcessRequestEventsRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        (self.plane, self.limit).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProcessRequestEventsRequestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (plane, limit) = <(ProcessRequestEventPlaneV1, u16)>::deserialize(deserializer)?;
        let request = Self { plane, limit };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
    }
}

/// Closed stage vocabulary; payloads and query text never enter diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessRequestEventStageV1 {
    Validated,
    ShuttingDown,
    QueueAdmitted,
    QueueRefusedGlobal,
    QueueRefusedRepository,
    DispatchStarted,
    BackendStarted,
    BackendReturned,
    BackendOutcome,
    ProviderStarted,
    ProviderReturned,
    IngestWindowStarted,
    IngestWindowReturned,
    DispatchReturned,
    PeerWatchFailed,
    PeerCancelled,
    ResponseEncodeFailed,
    ResponseWriteFailed,
    ResponseWritten,
    Aborted,
    Panicked,
}

impl ProcessRequestEventStageV1 {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Validated => "validated",
            Self::ShuttingDown => "shutting_down",
            Self::QueueAdmitted => "queue_admitted",
            Self::QueueRefusedGlobal => "queue_refused_global",
            Self::QueueRefusedRepository => "queue_refused_repository",
            Self::DispatchStarted => "dispatch_started",
            Self::BackendStarted => "backend_started",
            Self::BackendReturned => "backend_returned",
            Self::BackendOutcome => "backend_outcome",
            Self::ProviderStarted => "provider_started",
            Self::ProviderReturned => "provider_returned",
            Self::IngestWindowStarted => "ingest_window_started",
            Self::IngestWindowReturned => "ingest_window_returned",
            Self::DispatchReturned => "dispatch_returned",
            Self::PeerWatchFailed => "peer_watch_failed",
            Self::PeerCancelled => "peer_cancelled",
            Self::ResponseEncodeFailed => "response_encode_failed",
            Self::ResponseWriteFailed => "response_write_failed",
            Self::ResponseWritten => "response_written",
            Self::Aborted => "aborted",
            Self::Panicked => "panicked",
        }
    }
}

impl Serialize for ProcessRequestEventStageV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for ProcessRequestEventStageV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "validated" => Ok(Self::Validated),
            "shutting_down" => Ok(Self::ShuttingDown),
            "queue_admitted" => Ok(Self::QueueAdmitted),
            "queue_refused_global" => Ok(Self::QueueRefusedGlobal),
            "queue_refused_repository" => Ok(Self::QueueRefusedRepository),
            "dispatch_started" => Ok(Self::DispatchStarted),
            "backend_started" => Ok(Self::BackendStarted),
            "backend_returned" => Ok(Self::BackendReturned),
            "backend_outcome" => Ok(Self::BackendOutcome),
            "provider_started" => Ok(Self::ProviderStarted),
            "provider_returned" => Ok(Self::ProviderReturned),
            "ingest_window_started" => Ok(Self::IngestWindowStarted),
            "ingest_window_returned" => Ok(Self::IngestWindowReturned),
            "dispatch_returned" => Ok(Self::DispatchReturned),
            "peer_watch_failed" => Ok(Self::PeerWatchFailed),
            "peer_cancelled" => Ok(Self::PeerCancelled),
            "response_encode_failed" => Ok(Self::ResponseEncodeFailed),
            "response_write_failed" => Ok(Self::ResponseWriteFailed),
            "response_written" => Ok(Self::ResponseWritten),
            "aborted" => Ok(Self::Aborted),
            "panicked" => Ok(Self::Panicked),
            other => Err(de::Error::custom(format!(
                "unknown request-event stage {other:?}"
            ))),
        }
    }
}

/// A sequence-bearing event with stage-specific, closed diagnostic details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessRequestEventV1 {
    pub sequence: u64,
    pub request_id: NonZeroU64,
    pub connection_id: u64,
    pub stage: ProcessRequestEventStageV1,
    pub elapsed_micros: u64,
    pub route: Option<String>,
    pub error: Option<SearchPlaneErrorCodeV2>,
    pub ticket_id: Option<u64>,
    pub window_ordinal: Option<u64>,
}

impl ProcessRequestEventV1 {
    pub fn validate_v1(&self) -> Result<(), &'static str> {
        if self.sequence == 0 {
            return Err("request-event sequence is zero");
        }
        match self.stage {
            ProcessRequestEventStageV1::BackendOutcome => {
                let Some(route) = self.route.as_deref() else {
                    return Err("backend outcome lacks route");
                };
                if route.is_empty()
                    || route.len() > 64
                    || !route.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'_')
                    })
                {
                    return Err("backend outcome route is not bounded canonical ASCII");
                }
                if self.ticket_id.is_some() || self.window_ordinal.is_some() {
                    return Err("backend outcome carries unrelated detail");
                }
            }
            ProcessRequestEventStageV1::ProviderStarted
            | ProcessRequestEventStageV1::ProviderReturned => {
                if self.ticket_id.is_none()
                    || self.route.is_some()
                    || self.error.is_some()
                    || self.window_ordinal.is_some()
                {
                    return Err("provider stage detail is incomplete or mixed");
                }
            }
            ProcessRequestEventStageV1::IngestWindowStarted
            | ProcessRequestEventStageV1::IngestWindowReturned => {
                if self.window_ordinal.is_none()
                    || self.route.is_some()
                    || self.error.is_some()
                    || self.ticket_id.is_some()
                {
                    return Err("ingest-window stage detail is incomplete or mixed");
                }
            }
            _ => {
                if self.route.is_some()
                    || self.error.is_some()
                    || self.ticket_id.is_some()
                    || self.window_ordinal.is_some()
                {
                    return Err("plain request-event stage carries unrelated detail");
                }
            }
        }
        Ok(())
    }
}

impl Serialize for ProcessRequestEventV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        (
            self.sequence,
            self.request_id,
            self.connection_id,
            self.stage,
            self.elapsed_micros,
            &self.route,
            self.error,
            self.ticket_id,
            self.window_ordinal,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProcessRequestEventV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (
            sequence,
            request_id,
            connection_id,
            stage,
            elapsed_micros,
            route,
            error,
            ticket_id,
            window_ordinal,
        ) = <(
            u64,
            NonZeroU64,
            u64,
            ProcessRequestEventStageV1,
            u64,
            Option<String>,
            Option<SearchPlaneErrorCodeV2>,
            Option<u64>,
            Option<u64>,
        )>::deserialize(deserializer)?;
        let event = Self {
            sequence,
            request_id,
            connection_id,
            stage,
            elapsed_micros,
            route,
            error,
            ticket_id,
            window_ordinal,
        };
        event.validate_v1().map_err(de::Error::custom)?;
        Ok(event)
    }
}

/// One process-local, single-plane tail and its explicit loss provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessRequestEventsV1 {
    pub process_instance: String,
    pub plane: ProcessRequestEventPlaneV1,
    pub events: Vec<ProcessRequestEventV1>,
    pub oldest_retained_sequence: Option<u64>,
    pub next_sequence: u64,
    pub dropped_before: u64,
    pub dropped_after: u64,
    pub omitted_before_window: bool,
    pub sequence_exhausted: bool,
}

impl ProcessRequestEventsV1 {
    pub fn validate_v1(&self) -> Result<(), &'static str> {
        if self.process_instance.len() != 32
            || self.process_instance.bytes().all(|byte| byte == b'0')
            || !self
                .process_instance
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err("process-instance identity is not nonzero lowercase hex");
        }
        if self.events.len() > usize::from(MAX_PROCESS_REQUEST_EVENTS_V1)
            || self.next_sequence == 0
            || self.dropped_after < self.dropped_before
        {
            return Err("request-event window bounds are invalid");
        }
        if self.sequence_exhausted && self.next_sequence != u64::MAX {
            return Err("request-event sequence exhaustion is inconsistent");
        }
        match (self.events.first(), self.oldest_retained_sequence) {
            (None, None)
                if !self.omitted_before_window
                    && self.next_sequence == 1
                    && !self.sequence_exhausted => {}
            (Some(first), Some(oldest))
                if oldest > 0
                    && oldest <= first.sequence
                    && self.omitted_before_window == (first.sequence > oldest) => {}
            _ => return Err("request-event window origin is inconsistent"),
        }
        let mut previous = 0;
        for event in &self.events {
            event.validate_v1()?;
            if event.sequence <= previous
                || event.sequence >= self.next_sequence
                || (previous != 0 && event.sequence != previous + 1)
            {
                return Err("request-event sequence is reordered or out of range");
            }
            if let Some(route) = event.route.as_deref() {
                if !route.starts_with(self.plane.as_code_str())
                    || route.as_bytes().get(self.plane.as_code_str().len()) != Some(&b'.')
                {
                    return Err("backend route does not belong to reported plane");
                }
            }
            previous = event.sequence;
        }
        if let Some(last) = self.events.last()
            && last.sequence != self.next_sequence - 1
        {
            return Err("request-event tail does not reach next sequence");
        }
        Ok(())
    }

    /// This is separate from structural validation to avoid recursive
    /// serialization. Both producer and SDK consumer call it at their edge.
    pub fn validate_encoded_size_v1(&self) -> Result<(), String> {
        self.validate_v1().map_err(str::to_owned)?;
        let mut encoded = Vec::new();
        ciborium::into_writer(self, &mut encoded).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_PROCESS_REQUEST_EVENTS_WIRE_BYTES_V1 {
            return Err(format!(
                "request-event window exceeds {} encoded bytes",
                MAX_PROCESS_REQUEST_EVENTS_WIRE_BYTES_V1
            ));
        }
        Ok(())
    }
}

impl Serialize for ProcessRequestEventsV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        (
            &self.process_instance,
            self.plane,
            &self.events,
            self.oldest_retained_sequence,
            self.next_sequence,
            self.dropped_before,
            self.dropped_after,
            self.omitted_before_window,
            self.sequence_exhausted,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProcessRequestEventsV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (
            process_instance,
            plane,
            events,
            oldest_retained_sequence,
            next_sequence,
            dropped_before,
            dropped_after,
            omitted_before_window,
            sequence_exhausted,
        ) = <(
            String,
            ProcessRequestEventPlaneV1,
            Vec<ProcessRequestEventV1>,
            Option<u64>,
            u64,
            u64,
            u64,
            bool,
            bool,
        )>::deserialize(deserializer)?;
        let response = Self {
            process_instance,
            plane,
            events,
            oldest_retained_sequence,
            next_sequence,
            dropped_before,
            dropped_after,
            omitted_before_window,
            sequence_exhausted,
        };
        response.validate_v1().map_err(de::Error::custom)?;
        Ok(response)
    }
}

impl fmt::Display for ProcessRequestEventPlaneV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ProcessRequestEventsV1 {
        ProcessRequestEventsV1 {
            process_instance: "00000000000000000000000000000029".to_owned(),
            plane: ProcessRequestEventPlaneV1::Query,
            events: vec![ProcessRequestEventV1 {
                sequence: 7,
                request_id: NonZeroU64::new(17).expect("fixture request ID"),
                connection_id: 3,
                stage: ProcessRequestEventStageV1::BackendOutcome,
                elapsed_micros: 8,
                route: Some("query.text".to_owned()),
                error: None,
                ticket_id: None,
                window_ordinal: None,
            }],
            oldest_retained_sequence: Some(5),
            next_sequence: 8,
            dropped_before: 2,
            dropped_after: 2,
            omitted_before_window: true,
            sequence_exhausted: false,
        }
    }

    #[test]
    fn roundtrip_and_invalid_limit_are_fail_closed() {
        let request = ProcessRequestEventsRequestV1 {
            plane: ProcessRequestEventPlaneV1::Query,
            limit: 32,
        };
        let encoded = serde_json::to_string(&request).expect("request encode");
        assert_eq!(
            serde_json::from_str::<ProcessRequestEventsRequestV1>(&encoded)
                .expect("request decode"),
            request
        );
        assert!(serde_json::from_str::<ProcessRequestEventsRequestV1>("[\"query\",0]").is_err());
        assert!(serde_json::from_str::<ProcessRequestEventsRequestV1>("[\"query\",1025]").is_err());
        assert!(serde_json::from_str::<ProcessRequestEventsRequestV1>("[\"unknown\",1]").is_err());
        assert!(
            serde_json::from_str::<ProcessRequestEventsRequestV1>("[\"query\",1,true]").is_err()
        );
        let mut cbor = Vec::new();
        ciborium::into_writer(&request, &mut cbor).expect("request CBOR encode");
        let decoded: ProcessRequestEventsRequestV1 =
            ciborium::from_reader(cbor.as_slice()).expect("request CBOR decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn response_rejects_mixed_stage_details_and_forged_provenance() {
        let response = sample();
        response.validate_encoded_size_v1().expect("bounded sample");
        let encoded = serde_json::to_string(&response).expect("response encode");
        assert_eq!(
            serde_json::from_str::<ProcessRequestEventsV1>(&encoded).expect("response decode"),
            response
        );
        let mut cbor = Vec::new();
        ciborium::into_writer(&response, &mut cbor).expect("response CBOR encode");
        let decoded: ProcessRequestEventsV1 =
            ciborium::from_reader(cbor.as_slice()).expect("response CBOR decode");
        assert_eq!(decoded, response);
        assert!(
            ciborium::from_reader::<ProcessRequestEventsV1, _>(&cbor[..cbor.len() - 1]).is_err()
        );
        let mut forged = sample();
        forged.events[0].ticket_id = Some(5);
        assert!(forged.validate_v1().is_err());
        forged = sample();
        forged.events[0].route = Some("control.metrics".to_owned());
        assert!(forged.validate_v1().is_err());
        forged = sample();
        forged.dropped_after = 1;
        assert!(forged.validate_v1().is_err());
        forged = sample();
        forged.process_instance = "00000000000000000000000000000000".to_owned();
        assert!(forged.validate_v1().is_err());
        forged = sample();
        forged.next_sequence = 9;
        assert!(forged.validate_v1().is_err());
    }
}
