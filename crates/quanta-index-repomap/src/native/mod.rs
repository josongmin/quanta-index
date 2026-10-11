//! Native file/path inventory adapter.

mod file_map;
mod scratch;

pub use file_map::{
    FileMapCandidate, FileMapCapability, FileMapPlan, FileMapReadContext, FilePathEntry,
    FilePathPage, FilePathQuery, NativeFileMap, query_paths,
};
