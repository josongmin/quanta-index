use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::ManifestDigest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleMode {
    ServeOnly,
    IndexBuild,
}

impl BundleMode {
    const VARIANTS: &'static [&'static str] = &["ServeOnly", "IndexBuild"];

    const fn as_wire_str(self) -> &'static str {
        match self {
            Self::ServeOnly => "ServeOnly",
            Self::IndexBuild => "IndexBuild",
        }
    }
}

impl Serialize for BundleMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_wire_str())
    }
}

struct BundleModeVisitor;

impl Visitor<'_> for BundleModeVisitor {
    type Value = BundleMode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BundleMode variant string (ServeOnly|IndexBuild)")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "ServeOnly" => Ok(BundleMode::ServeOnly),
            "IndexBuild" => Ok(BundleMode::IndexBuild),
            other => Err(de::Error::unknown_variant(other, BundleMode::VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for BundleMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(BundleModeVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleEncoding {
    ArrowIpc,
    Feather,
    Json,
    RawF32,
    TantivyDirectory,
    LanceDirectory,
    Opaque,
}

impl BundleEncoding {
    const VARIANTS: &'static [&'static str] = &[
        "ArrowIpc",
        "Feather",
        "Json",
        "RawF32",
        "TantivyDirectory",
        "LanceDirectory",
        "Opaque",
    ];

    const fn as_wire_str(self) -> &'static str {
        match self {
            Self::ArrowIpc => "ArrowIpc",
            Self::Feather => "Feather",
            Self::Json => "Json",
            Self::RawF32 => "RawF32",
            Self::TantivyDirectory => "TantivyDirectory",
            Self::LanceDirectory => "LanceDirectory",
            Self::Opaque => "Opaque",
        }
    }
}

impl Serialize for BundleEncoding {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_wire_str())
    }
}

struct BundleEncodingVisitor;

impl Visitor<'_> for BundleEncodingVisitor {
    type Value = BundleEncoding;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BundleEncoding variant string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "ArrowIpc" => Ok(BundleEncoding::ArrowIpc),
            "Feather" => Ok(BundleEncoding::Feather),
            "Json" => Ok(BundleEncoding::Json),
            "RawF32" => Ok(BundleEncoding::RawF32),
            "TantivyDirectory" => Ok(BundleEncoding::TantivyDirectory),
            "LanceDirectory" => Ok(BundleEncoding::LanceDirectory),
            "Opaque" => Ok(BundleEncoding::Opaque),
            other => Err(de::Error::unknown_variant(other, BundleEncoding::VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for BundleEncoding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(BundleEncodingVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleArtifactRef {
    pub relative_path: String,
    pub encoding: BundleEncoding,
    pub byte_length: u64,
    pub content_digest: ManifestDigest,
}

const BUNDLE_ARTIFACT_REF_FIELDS: &[&str] =
    &["relative_path", "encoding", "byte_length", "content_digest"];

impl Serialize for BundleArtifactRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BundleArtifactRef", 4)?;
        state.serialize_field("relative_path", &self.relative_path)?;
        state.serialize_field("encoding", &self.encoding)?;
        state.serialize_field("byte_length", &self.byte_length)?;
        state.serialize_field("content_digest", &self.content_digest)?;
        state.end()
    }
}

struct BundleArtifactRefVisitor;

impl<'de> Visitor<'de> for BundleArtifactRefVisitor {
    type Value = BundleArtifactRef;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BundleArtifactRef map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut relative_path: Option<String> = None;
        let mut encoding: Option<BundleEncoding> = None;
        let mut byte_length: Option<u64> = None;
        let mut content_digest: Option<ManifestDigest> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "relative_path" => {
                    if relative_path.is_some() {
                        return Err(de::Error::duplicate_field("relative_path"));
                    }
                    relative_path = Some(map.next_value()?);
                }
                "encoding" => {
                    if encoding.is_some() {
                        return Err(de::Error::duplicate_field("encoding"));
                    }
                    encoding = Some(map.next_value()?);
                }
                "byte_length" => {
                    if byte_length.is_some() {
                        return Err(de::Error::duplicate_field("byte_length"));
                    }
                    byte_length = Some(map.next_value()?);
                }
                "content_digest" => {
                    if content_digest.is_some() {
                        return Err(de::Error::duplicate_field("content_digest"));
                    }
                    content_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, BUNDLE_ARTIFACT_REF_FIELDS));
                }
            }
        }
        let relative_path =
            relative_path.ok_or_else(|| de::Error::missing_field("relative_path"))?;
        let encoding = encoding.ok_or_else(|| de::Error::missing_field("encoding"))?;
        let byte_length = byte_length.ok_or_else(|| de::Error::missing_field("byte_length"))?;
        let content_digest =
            content_digest.ok_or_else(|| de::Error::missing_field("content_digest"))?;
        Ok(BundleArtifactRef {
            relative_path,
            encoding,
            byte_length,
            content_digest,
        })
    }
}

impl<'de> Deserialize<'de> for BundleArtifactRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "BundleArtifactRef",
            BUNDLE_ARTIFACT_REF_FIELDS,
            BundleArtifactRefVisitor,
        )
    }
}
