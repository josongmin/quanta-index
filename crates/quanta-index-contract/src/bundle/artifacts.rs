use crate::ManifestDigest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleMode {
    ServeOnly,
    IndexBuild,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleArtifactRef {
    pub relative_path: String,
    pub encoding: BundleEncoding,
    pub byte_length: u64,
    pub content_digest: ManifestDigest,
}
