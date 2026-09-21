//! Audit log entry shape per OBS-01 § 4.5.
//!
//! Every audit row records `tenant_id`, `user_id`, `action`, `resource`,
//! `outcome` (granted / denied / error), plus a millisecond timestamp.
//! The audit sink is **separate** from the operational sink per
//! RFC § Security and Authz Model item 3.
//!
//! D18 — hand-rolled serde, no proc-macro derives.

use core::fmt;

use crate::errors::{ObsError, ObsErrorCode};

/// Closed taxonomy of audit outcomes per OBS-01 § 4.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AuditOutcome {
    /// Authz path completed and the action was permitted.
    Granted,
    /// Authz path completed and the action was rejected.
    Denied,
    /// Authz path crashed or the resource was unavailable.
    Error,
}

impl AuditOutcome {
    /// `snake_case` wire form.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::Error => "error",
        }
    }

    /// Inverse of [`AuditOutcome::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "granted" => Self::Granted,
            "denied" => Self::Denied,
            "error" => Self::Error,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for AuditOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for AuditOutcome {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for AuditOutcome {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = AuditOutcome;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("AuditOutcome snake_case string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<AuditOutcome, E> {
                AuditOutcome::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<AuditOutcome>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// One audit row. Five required identity fields plus a typed outcome and a
/// millisecond timestamp anchor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AuditEntry {
    /// Opaque tenant id.
    pub tenant_id: Box<str>,
    /// Opaque user id.
    pub user_id: Box<str>,
    /// What was attempted (`query`, `delete`, `materialize`, …).
    pub action: Box<str>,
    /// What was acted on (a query hash, a repo id, a manifest gen, …).
    pub resource: Box<str>,
    /// Authz outcome.
    pub outcome: AuditOutcome,
    /// Audit row timestamp, milliseconds since UNIX epoch.
    pub at_ms: u64,
}

impl AuditEntry {
    /// Minimal constructor; the call site is responsible for running
    /// [`validate_audit`] before emit.
    #[must_use]
    pub fn new(
        tenant_id: impl Into<Box<str>>,
        user_id: impl Into<Box<str>>,
        action: impl Into<Box<str>>,
        resource: impl Into<Box<str>>,
        outcome: AuditOutcome,
        at_ms: u64,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            user_id: user_id.into(),
            action: action.into(),
            resource: resource.into(),
            outcome,
            at_ms,
        }
    }
}

/// Structural validation. Returns
/// [`crate::errors::ObsErrorCode::ObsAuditMissingField`] on any empty
/// required field. The audit sink must NEVER write a row that has not
/// been through this validator.
pub fn validate_audit(e: &AuditEntry) -> Result<(), ObsError> {
    if e.tenant_id.is_empty() {
        return Err(ObsError::new(
            ObsErrorCode::ObsAuditMissingField,
            "tenant_id must not be empty",
        ));
    }
    if e.user_id.is_empty() {
        return Err(ObsError::new(ObsErrorCode::ObsAuditMissingField, "user_id must not be empty"));
    }
    if e.action.is_empty() {
        return Err(ObsError::new(ObsErrorCode::ObsAuditMissingField, "action must not be empty"));
    }
    if e.resource.is_empty() {
        return Err(ObsError::new(
            ObsErrorCode::ObsAuditMissingField,
            "resource must not be empty",
        ));
    }
    Ok(())
}

impl serde::Serialize for AuditEntry {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("AuditEntry", 6)?;
        st.serialize_field("tenant_id", self.tenant_id.as_ref())?;
        st.serialize_field("user_id", self.user_id.as_ref())?;
        st.serialize_field("action", self.action.as_ref())?;
        st.serialize_field("resource", self.resource.as_ref())?;
        st.serialize_field("outcome", &self.outcome)?;
        st.serialize_field("at_ms", &self.at_ms)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for AuditEntry {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            TenantId,
            UserId,
            Action,
            Resource,
            Outcome,
            AtMs,
        }
        impl<'de2> serde::Deserialize<'de2> for Field {
            fn deserialize<D2>(de: D2) -> Result<Self, D2::Error>
            where
                D2: serde::Deserializer<'de2>,
            {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Field;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("AuditEntry field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "tenant_id" => Ok(Field::TenantId),
                            "user_id" => Ok(Field::UserId),
                            "action" => Ok(Field::Action),
                            "resource" => Ok(Field::Resource),
                            "outcome" => Ok(Field::Outcome),
                            "at_ms" => Ok(Field::AtMs),
                            other => Err(E::unknown_field(
                                other,
                                &[
                                    "tenant_id",
                                    "user_id",
                                    "action",
                                    "resource",
                                    "outcome",
                                    "at_ms",
                                ],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct AV;
        impl<'d> serde::de::Visitor<'d> for AV {
            type Value = AuditEntry;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("AuditEntry struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<AuditEntry, A::Error> {
                let mut tenant_id: Option<Box<str>> = None;
                let mut user_id: Option<Box<str>> = None;
                let mut action: Option<Box<str>> = None;
                let mut resource: Option<Box<str>> = None;
                let mut outcome: Option<AuditOutcome> = None;
                let mut at_ms: Option<u64> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::TenantId => {
                            if tenant_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("tenant_id"));
                            }
                            let v: String = map.next_value()?;
                            tenant_id = Some(v.into_boxed_str());
                        }
                        Field::UserId => {
                            if user_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("user_id"));
                            }
                            let v: String = map.next_value()?;
                            user_id = Some(v.into_boxed_str());
                        }
                        Field::Action => {
                            if action.is_some() {
                                return Err(serde::de::Error::duplicate_field("action"));
                            }
                            let v: String = map.next_value()?;
                            action = Some(v.into_boxed_str());
                        }
                        Field::Resource => {
                            if resource.is_some() {
                                return Err(serde::de::Error::duplicate_field("resource"));
                            }
                            let v: String = map.next_value()?;
                            resource = Some(v.into_boxed_str());
                        }
                        Field::Outcome => {
                            if outcome.is_some() {
                                return Err(serde::de::Error::duplicate_field("outcome"));
                            }
                            outcome = Some(map.next_value()?);
                        }
                        Field::AtMs => {
                            if at_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field("at_ms"));
                            }
                            at_ms = Some(map.next_value()?);
                        }
                    }
                }
                Ok(AuditEntry {
                    tenant_id: tenant_id
                        .ok_or_else(|| serde::de::Error::missing_field("tenant_id"))?,
                    user_id: user_id.ok_or_else(|| serde::de::Error::missing_field("user_id"))?,
                    action: action.ok_or_else(|| serde::de::Error::missing_field("action"))?,
                    resource: resource
                        .ok_or_else(|| serde::de::Error::missing_field("resource"))?,
                    outcome: outcome.ok_or_else(|| serde::de::Error::missing_field("outcome"))?,
                    at_ms: at_ms.ok_or_else(|| serde::de::Error::missing_field("at_ms"))?,
                })
            }
        }

        de.deserialize_struct(
            "AuditEntry",
            &[
                "tenant_id",
                "user_id",
                "action",
                "resource",
                "outcome",
                "at_ms",
            ],
            AV,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{AuditEntry, AuditOutcome, validate_audit};
    use crate::errors::ObsErrorCode;

    const ALL: &[AuditOutcome] = &[
        AuditOutcome::Granted,
        AuditOutcome::Denied,
        AuditOutcome::Error,
    ];

    fn ok_entry() -> AuditEntry {
        AuditEntry::new("tenant-1", "user-7", "query", "0xdeadbeef", AuditOutcome::Granted, 123)
    }

    #[test]
    fn outcome_strs_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for o in ALL {
            let s = o.as_code_str();
            assert!(!seen.contains(&s));
            seen.push(s);
        }
    }

    #[test]
    fn outcome_roundtrip() {
        for o in ALL {
            assert_eq!(AuditOutcome::from_code_str(o.as_code_str()), Some(*o));
        }
    }

    #[test]
    fn outcome_unknown_returns_none() {
        assert!(AuditOutcome::from_code_str("granted_v2").is_none());
    }

    #[test]
    fn good_entry_validates() {
        match validate_audit(&ok_entry()) {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn empty_tenant_id_rejected() {
        let mut e = ok_entry();
        e.tenant_id = Box::from("");
        match validate_audit(&e) {
            Ok(()) => assert!(false, "must reject"),
            Err(err) => assert_eq!(err.code, ObsErrorCode::ObsAuditMissingField),
        }
    }

    #[test]
    fn empty_user_id_rejected() {
        let mut e = ok_entry();
        e.user_id = Box::from("");
        match validate_audit(&e) {
            Ok(()) => assert!(false, "must reject"),
            Err(err) => assert_eq!(err.code, ObsErrorCode::ObsAuditMissingField),
        }
    }

    #[test]
    fn empty_action_rejected() {
        let mut e = ok_entry();
        e.action = Box::from("");
        match validate_audit(&e) {
            Ok(()) => assert!(false, "must reject"),
            Err(err) => assert_eq!(err.code, ObsErrorCode::ObsAuditMissingField),
        }
    }

    #[test]
    fn empty_resource_rejected() {
        let mut e = ok_entry();
        e.resource = Box::from("");
        match validate_audit(&e) {
            Ok(()) => assert!(false, "must reject"),
            Err(err) => assert_eq!(err.code, ObsErrorCode::ObsAuditMissingField),
        }
    }

    #[test]
    fn audit_entry_serde_roundtrip() {
        let e = ok_entry();
        let buf = match serde_json::to_vec(&e) {
            Ok(v) => v,
            Err(err) => {
                assert!(false, "ser: {err}");
                return;
            }
        };
        match serde_json::from_slice::<AuditEntry>(&buf) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "de: {err}"),
        }
    }
}
