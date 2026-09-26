//! Preview provenance, independent of ranked-hit identity and completeness.
//! Ranges are chunk-relative byte intervals; a file offset exists only when a
//! verified chunk base and immutable source identity are supplied together.

use crate::SourceFileRevision;
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    SourceChunk,
    Path,
    SyntheticSymbolLabel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewUnavailableReason {
    NoPositiveWitness,
    SourceNotProvided,
    WorkBudget,
    FocusExceedsBudget,
    UnsupportedRange,
}

macro_rules! preview_string_enum {
    ($ty:ident, $visitor:ident, {$($variant:ident => $wire:literal),+ $(,)?}) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(match self { $(Self::$variant => $wire),+ })
            }
        }
        struct $visitor;
        impl Visitor<'_> for $visitor {
            type Value = $ty;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(stringify!($ty)) }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<$ty, E> {
                match value { $($wire => Ok($ty::$variant),)+ other => Err(de::Error::unknown_variant(other, &[$($wire),+])) }
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> { deserializer.deserialize_str($visitor) }
        }
    };
}
preview_string_enum!(PreviewKind, PreviewKindVisitor, {
    SourceChunk => "source_chunk", Path => "path", SyntheticSymbolLabel => "synthetic_symbol_label",
});
preview_string_enum!(PreviewUnavailableReason, PreviewUnavailableVisitor, {
    NoPositiveWitness => "no_positive_witness", SourceNotProvided => "source_not_provided",
    WorkBudget => "work_budget", FocusExceedsBudget => "focus_exceeds_budget", UnsupportedRange => "unsupported_range",
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewByteRange {
    pub start: u64,
    pub end: u64,
}

impl PreviewByteRange {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.start >= self.end {
            return Err("preview range must be nonempty and ordered");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewMetadata {
    pub kind: PreviewKind,
    pub source: Option<SourceFileRevision>,
    pub chunk_start_byte: Option<u64>,
    pub original_focus: Option<PreviewByteRange>,
    pub original_context: Option<PreviewByteRange>,
    pub normalized_focus: Option<PreviewByteRange>,
    pub normalization_equivalent: bool,
    pub unavailable_reason: Option<PreviewUnavailableReason>,
}

impl PreviewMetadata {
    /// Validate shape only. The render/read owner must prove byte boundaries,
    /// immutable source/hash, normalized equivalence and actual emitted bytes.
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Some(source) = &self.source {
            source.validate()?;
        }
        if self.chunk_start_byte.is_some() && self.source.is_none() {
            return Err("chunk base requires source identity");
        }
        if self.unavailable_reason.is_some() || self.kind != PreviewKind::SourceChunk {
            if self.original_focus.is_some()
                || self.original_context.is_some()
                || self.normalized_focus.is_some()
                || self.normalization_equivalent
            {
                return Err("unavailable/path/synthetic previews cannot claim source spans");
            }
            if self.unavailable_reason.is_none() && self.source.is_none() {
                return Err("available preview requires source identity");
            }
            return Ok(());
        }
        if self.source.is_none() {
            return Err("source excerpt requires source identity");
        }
        let focus = self
            .original_focus
            .ok_or("source excerpt requires original focus")?;
        let context = self
            .original_context
            .ok_or("source excerpt requires original context")?;
        let normalized = self
            .normalized_focus
            .ok_or("source excerpt requires normalized focus")?;
        focus.validate()?;
        context.validate()?;
        normalized.validate()?;
        if context.start > focus.start || context.end < focus.end {
            return Err("source context must contain the complete focus");
        }
        if let Some(base) = self.chunk_start_byte {
            let _file_end = base
                .checked_add(context.end)
                .ok_or("source file byte offset overflow")?;
        }
        Ok(())
    }

    #[must_use]
    pub fn unavailable(
        kind: PreviewKind,
        reason: PreviewUnavailableReason,
        source: Option<SourceFileRevision>,
    ) -> Self {
        Self {
            kind,
            source,
            chunk_start_byte: None,
            original_focus: None,
            original_context: None,
            normalized_focus: None,
            normalization_equivalent: false,
            unavailable_reason: Some(reason),
        }
    }
}

macro_rules! preview_struct_codec {
    ($ty:ident, $visitor:ident, $fields:ident, {$($field:ident: $field_ty:ty),+ $(,)?}) => {
        const $fields: &[&str] = &[$(stringify!($field)),+];
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.validate().map_err(serde::ser::Error::custom)?;
                let mut state = serializer.serialize_struct(stringify!($ty), $fields.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }
        struct $visitor;
        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(stringify!($ty)) }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<$ty, A::Error> {
                $(let mut $field: Option<$field_ty> = None;)+
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        $(stringify!($field) => {
                            if $field.is_some() { return Err(de::Error::duplicate_field(stringify!($field))); }
                            $field = Some(map.next_value()?);
                        })+
                        other => return Err(de::Error::unknown_field(other, $fields)),
                    }
                }
                let value = $ty { $($field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,)+ };
                value.validate().map_err(de::Error::custom)?;
                Ok(value)
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}
preview_struct_codec!(PreviewByteRange, PreviewRangeVisitor, PREVIEW_RANGE_FIELDS, {start: u64, end: u64});
preview_struct_codec!(PreviewMetadata, PreviewMetadataVisitor, PREVIEW_METADATA_FIELDS, {
    kind: PreviewKind, source: Option<SourceFileRevision>, chunk_start_byte: Option<u64>,
    original_focus: Option<PreviewByteRange>, original_context: Option<PreviewByteRange>,
    normalized_focus: Option<PreviewByteRange>, normalization_equivalent: bool,
    unavailable_reason: Option<PreviewUnavailableReason>,
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RepoId, RepoRelativePath, RevisionId, SourceFileKey};
    fn preview() -> PreviewMetadata {
        PreviewMetadata {
            kind: PreviewKind::SourceChunk,
            source: Some(SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source").expect("repo"),
                    repo_relative_path: RepoRelativePath::new("a.rs"),
                },
                revision_id: RevisionId::new("r").expect("revision"),
                source_sha256: [1; 32],
            }),
            chunk_start_byte: Some(100),
            original_focus: Some(PreviewByteRange { start: 2, end: 4 }),
            original_context: Some(PreviewByteRange { start: 0, end: 6 }),
            normalized_focus: Some(PreviewByteRange { start: 2, end: 3 }),
            normalization_equivalent: true,
            unavailable_reason: None,
        }
    }
    #[test]
    fn source_preview_roundtrip() {
        let value = preview();
        let raw = serde_json::to_string(&value).expect("encode");
        assert_eq!(
            serde_json::from_str::<PreviewMetadata>(&raw).expect("decode"),
            value
        );
    }
    #[test]
    fn no_false_source_spans_or_unbound_source() {
        for kind in [PreviewKind::Path, PreviewKind::SyntheticSymbolLabel] {
            let mut value = preview();
            value.kind = kind;
            assert!(value.validate().is_err());
        }
        let mut value = preview();
        value.source = None;
        assert!(value.validate().is_err());
        let mut value = preview();
        value.original_context = Some(PreviewByteRange { start: 0, end: 3 });
        assert!(value.validate().is_err());
        let mut value = preview();
        value.chunk_start_byte = Some(u64::MAX);
        assert!(value.validate().is_err());
    }
    #[test]
    fn unavailable_has_no_successful_focus() {
        let mut value = PreviewMetadata::unavailable(
            PreviewKind::SourceChunk,
            PreviewUnavailableReason::WorkBudget,
            None,
        );
        assert!(value.validate().is_ok());
        value.original_focus = Some(PreviewByteRange { start: 0, end: 1 });
        assert!(serde_json::to_string(&value).is_err());
    }
    #[test]
    fn wire_refuses_missing_unknown_duplicate_and_bad_intervals() {
        let mut value = serde_json::to_value(preview()).expect("encode");
        let removed = value.as_object_mut().expect("map").remove("source");
        assert!(removed.is_some());
        assert!(serde_json::from_value::<PreviewMetadata>(value).is_err());
        for raw in [
            r#"{"start":0,"end":0}"#,
            r#"{"start":2,"end":1}"#,
            r#"{"start":0,"start":1,"end":2}"#,
            r#"{"start":0,"end":2,"other":0}"#,
        ] {
            assert!(serde_json::from_str::<PreviewByteRange>(raw).is_err());
        }
    }
}
