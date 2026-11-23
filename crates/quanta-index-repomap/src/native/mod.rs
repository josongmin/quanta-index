//! Native file/path inventory adapter.

mod file_map;
pub mod graph;
mod legacy_import;
mod path_reader;
mod repository_path_reader;
mod scratch;

pub use file_map::{
    FileMapCandidate, FileMapCapability, FileMapPlan, FileMapReadContext, FilePathEntry,
    FilePathPage, FilePathQuery, NativeFileMap, query_paths,
};
pub use legacy_import::{
    LoadedLegacyRepoMapRowV1, NATIVE_LEGACY_REPOMAP_FORMAT_V1, NATIVE_LEGACY_REPOMAP_KIND_V1,
    NATIVE_LEGACY_REPOMAP_SCHEMA_V1, NativeLegacyRepoMapBuildV1, NativeLegacyRepoMapReadV1,
    VerifiedLegacyRepoMapImportV1, load_verified_legacy_repomap_import_v1,
    with_verified_native_legacy_repomap_member_v1,
};
pub use path_reader::{NativePathReader, PathReadRequest};
pub use repository_path_reader::{
    NativeRepositoryPathReader, RepositoryFilePathPage, RepositoryPathReadRequest,
};

pub use graph::{
    GraphPage, GraphQuery, NativeGraph, NativeGraphReader, NativeRepoMapPriorEndpointOwnerV1,
    NativeRepoMapPriorEndpointOwnersV1, NativeRepoMapPriorEndpointReaderV1,
    NativeRepoMapPriorFileContextV1, NativeRepoMapPriorFileReaderV1,
};
