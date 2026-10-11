//! Component document IDs are allocated by the existing source-event catalog.
//!
//! Lexical writers consume its original durable ranges and never recover a
//! counter from current documents, generation paths or a branch watermark.

pub(crate) use quanta_index_core::LexicalDocumentVersionRangeV1 as DocVersionReservation;
