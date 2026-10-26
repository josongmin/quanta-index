//! Durable immutable objects and bounded persistent maps for the native index.
//!
//! This crate stores candidates. The catalog remains the sole authority for
//! publication, reachability, tickets, and physical reclamation.

mod map;
mod object;
mod scratch;

pub use object::{
    FileObjectReader, FileObjectStore, ManagedObjectIo, ObjectIoObservation, ObjectIoSnapshot,
};
pub use scratch::FileScratchSpace;
