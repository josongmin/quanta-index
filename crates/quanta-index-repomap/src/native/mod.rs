//! Native file/path inventory adapter.

mod file_map;
pub mod graph;
mod path_reader;
mod scratch;

pub use file_map::{
    FileMapCandidate, FileMapCapability, FileMapPlan, FileMapReadContext, FilePathEntry,
    FilePathPage, FilePathQuery, NativeFileMap, query_paths,
};
pub use path_reader::{NativePathReader, PathReadRequest};

pub use graph::{
    GraphPage, GraphQuery, NativeGraph, NativeGraphReader, NativeRepoMapRankPage,
    NativeRepoMapRankReader,
};
