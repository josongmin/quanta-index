//! Durable number allocation, separate from publication and activation CAS.

use quanta_index_contract_base::ACTIVATION_ROOT_INCARNATION_BYTES_V1;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{ManifestGeneration, RepoId, RevisionId, SourcePublicationEvent};

/// Stable source intent before a final batch has a generation or journal key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationReservationRequestV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub source_event: SourcePublicationEvent,
}

impl SearchCorpusGenerationReservationRequestV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        self.source_event
            .validate()
            .map_err(|error| format!("generation reservation: {error}"))
    }
}

impl Serialize for SearchCorpusGenerationReservationRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        (&self.repo_id, &self.revision_id, &self.source_event).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SearchCorpusGenerationReservationRequestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (repo_id, revision_id, source_event) =
            <(RepoId, RevisionId, SourcePublicationEvent)>::deserialize(deserializer)?;
        let request = Self {
            repo_id,
            revision_id,
            source_event,
        };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
    }
}

/// Original durable allocation. This is neither a publish nor activation ACK.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationReservationV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub source_event: SourcePublicationEvent,
    pub generation: ManifestGeneration,
    pub root_incarnation: [u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
}

impl SearchCorpusGenerationReservationV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        self.source_event
            .validate()
            .map_err(|error| format!("generation reservation: {error}"))?;
        if self.generation.get() == 0 {
            return Err("generation reservation requires a nonzero generation".into());
        }
        if self.root_incarnation == [0; ACTIVATION_ROOT_INCARNATION_BYTES_V1] {
            return Err("generation reservation requires a nonzero root incarnation".into());
        }
        Ok(())
    }
}

impl Serialize for SearchCorpusGenerationReservationV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        (
            &self.repo_id,
            &self.revision_id,
            &self.source_event,
            self.generation,
            self.root_incarnation,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SearchCorpusGenerationReservationV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (repo_id, revision_id, source_event, generation, root_incarnation) =
            <(
                RepoId,
                RevisionId,
                SourcePublicationEvent,
                ManifestGeneration,
                [u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
            )>::deserialize(deserializer)?;
        let reservation = Self {
            repo_id,
            revision_id,
            source_event,
            generation,
            root_incarnation,
        };
        reservation.validate_v1().map_err(de::Error::custom)?;
        Ok(reservation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::Value;
    use serde_json::json;

    fn request() -> SearchCorpusGenerationReservationRequestV1 {
        SearchCorpusGenerationReservationRequestV1 {
            repo_id: RepoId::new("repo-a").expect("valid repo"),
            revision_id: RevisionId::new("rev-a").expect("valid revision"),
            source_event: SourcePublicationEvent {
                stream_id: "stream-a".to_string(),
                event_id: "event-a".to_string(),
                expected_base_event_id: None,
                payload_sha256: [1; 32],
            },
        }
    }

    fn response() -> SearchCorpusGenerationReservationV1 {
        let request = request();
        SearchCorpusGenerationReservationV1 {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            source_event: request.source_event,
            generation: ManifestGeneration::new(9),
            root_incarnation: [2; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
        }
    }

    fn event_json() -> serde_json::Value {
        json!({
            "stream_id": "stream-a",
            "event_id": "event-a",
            "expected_base_event_id": null,
            "payload_sha256": vec![1u8; 32],
        })
    }

    fn event_cbor() -> Value {
        event_cbor_with_id("event-a")
    }

    fn event_cbor_with_id(event_id: &str) -> Value {
        Value::Map(vec![
            (
                Value::Text("stream_id".into()),
                Value::Text("stream-a".into()),
            ),
            (Value::Text("event_id".into()), Value::Text(event_id.into())),
            (Value::Text("expected_base_event_id".into()), Value::Null),
            (
                Value::Text("payload_sha256".into()),
                Value::Array(vec![Value::Integer(1_u64.into()); 32]),
            ),
        ])
    }

    fn cbor_bytes(value: &Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes).expect("fixture CBOR encodes");
        bytes
    }

    #[test]
    fn reservation_fixed_tuple_json_and_cbor_roundtrip_v1() {
        let request = request();
        let response = response();
        let request_json = json!(["repo-a", "rev-a", event_json()]);
        let response_json = json!(["repo-a", "rev-a", event_json(), 9, vec![2u8; 16]]);
        assert_eq!(
            serde_json::to_value(&request).expect("request JSON"),
            request_json
        );
        assert_eq!(
            serde_json::to_value(&response).expect("response JSON"),
            response_json
        );
        assert_eq!(
            serde_json::from_value::<SearchCorpusGenerationReservationRequestV1>(request_json)
                .expect("request JSON roundtrip"),
            request
        );
        assert_eq!(
            serde_json::from_value::<SearchCorpusGenerationReservationV1>(response_json)
                .expect("response JSON roundtrip"),
            response
        );

        let request_cbor = Value::Array(vec![
            Value::Text("repo-a".into()),
            Value::Text("rev-a".into()),
            event_cbor(),
        ]);
        let response_cbor = Value::Array(vec![
            Value::Text("repo-a".into()),
            Value::Text("rev-a".into()),
            event_cbor(),
            Value::Integer(9_u64.into()),
            Value::Array(vec![Value::Integer(2_u64.into()); 16]),
        ]);
        for (actual, expected) in [
            (
                cbor_bytes_checked(&request).expect("request CBOR"),
                request_cbor.clone(),
            ),
            (
                cbor_bytes_checked(&response).expect("response CBOR"),
                response_cbor.clone(),
            ),
        ] {
            assert_eq!(
                ciborium::from_reader::<Value, _>(actual.as_slice()).expect("CBOR value"),
                expected
            );
        }
        assert_eq!(
            ciborium::from_reader::<SearchCorpusGenerationReservationRequestV1, _>(
                cbor_bytes(&request_cbor).as_slice()
            )
            .expect("request CBOR roundtrip"),
            request
        );
        assert_eq!(
            ciborium::from_reader::<SearchCorpusGenerationReservationV1, _>(
                cbor_bytes(&response_cbor).as_slice()
            )
            .expect("response CBOR roundtrip"),
            response
        );
    }

    #[test]
    fn reservation_rejects_malformed_generation_incarnation_and_event_v1() {
        let malformed_request_json = json!(["repo-a", "rev-a", {
            "stream_id": "stream-a", "event_id": "bad event",
            "expected_base_event_id": null, "payload_sha256": vec![1u8; 32]
        }]);
        assert!(
            serde_json::from_value::<SearchCorpusGenerationReservationRequestV1>(
                malformed_request_json
            )
            .is_err()
        );
        for malformed in [
            json!(["repo-a", "rev-a", event_json(), 0, vec![2u8; 16]]),
            json!(["repo-a", "rev-a", event_json(), 9, vec![0u8; 16]]),
            json!(["repo-a", "rev-a", event_json(), 9, vec![2u8; 15]]),
            json!(["repo-a", "rev-a", event_json(), 9, vec![2u8; 16], "extra"]),
            json!(["repo-a", "rev-a", {
                "stream_id": "bad stream", "event_id": "event-a",
                "expected_base_event_id": null, "payload_sha256": vec![1u8; 32]
            }, 9, vec![2u8; 16]]),
        ] {
            assert!(
                serde_json::from_value::<SearchCorpusGenerationReservationV1>(malformed).is_err()
            );
        }
        let mut invalid_generation = response();
        invalid_generation.generation = ManifestGeneration::new(0);
        assert!(cbor_bytes_checked(&invalid_generation).is_err());
        let mut invalid_root = response();
        invalid_root.root_incarnation = [0; ACTIVATION_ROOT_INCARNATION_BYTES_V1];
        assert!(cbor_bytes_checked(&invalid_root).is_err());

        for (generation, root) in [
            (
                Value::Integer(0_u64.into()),
                Value::Array(vec![Value::Integer(2_u64.into()); 16]),
            ),
            (
                Value::Integer(9_u64.into()),
                Value::Array(vec![Value::Integer(0_u64.into()); 16]),
            ),
            (
                Value::Integer(9_u64.into()),
                Value::Array(vec![Value::Integer(2_u64.into()); 15]),
            ),
        ] {
            let malformed_cbor = Value::Array(vec![
                Value::Text("repo-a".into()),
                Value::Text("rev-a".into()),
                event_cbor(),
                generation,
                root,
            ]);
            assert!(
                ciborium::from_reader::<SearchCorpusGenerationReservationV1, _>(
                    cbor_bytes(&malformed_cbor).as_slice()
                )
                .is_err()
            );
        }

        let malformed = event_cbor_with_id("bad event");
        let request_cbor = Value::Array(vec![
            Value::Text("repo-a".into()),
            Value::Text("rev-a".into()),
            malformed.clone(),
        ]);
        assert!(
            ciborium::from_reader::<SearchCorpusGenerationReservationRequestV1, _>(
                cbor_bytes(&request_cbor).as_slice()
            )
            .is_err()
        );
        let response_cbor = Value::Array(vec![
            Value::Text("repo-a".into()),
            Value::Text("rev-a".into()),
            malformed,
            Value::Integer(9_u64.into()),
            Value::Array(vec![Value::Integer(2_u64.into()); 16]),
        ]);
        assert!(
            ciborium::from_reader::<SearchCorpusGenerationReservationV1, _>(
                cbor_bytes(&response_cbor).as_slice()
            )
            .is_err()
        );
    }

    fn cbor_bytes_checked<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes).map_err(|error| error.to_string())?;
        Ok(bytes)
    }
}
