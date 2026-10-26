// SPDX-License-Identifier: Apache-2.0

//! Serde deserialization support for CBOR

mod codec;
mod error;

pub use codec::{
    from_reader, from_reader_with_buffer, from_reader_with_recursion_limit,
    from_slice_with_finite_error_and_work_v3, from_slice_with_finite_seed_and_allocation_v3,
    FiniteParserAllocationAdmissionV3, FiniteParserAllocationRefusalV3, FiniteReadErrorV3,
};
pub use error::{Error, FiniteSemanticReasonV3, SemanticReasonV3};
