// SPDX-License-Identifier: Apache-2.0

//! Serde serialization support for CBOR

mod codec;
mod error;

pub use codec::into_writer;
pub use error::Error;
