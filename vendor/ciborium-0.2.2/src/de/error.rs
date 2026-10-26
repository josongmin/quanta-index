// SPDX-License-Identifier: Apache-2.0

use alloc::string::{String, ToString};
use core::fmt::{Debug, Display, Formatter, Result};

use serde::de::{Error as DeError, StdError};

/// Selects the semantic error payload before Serde visits the input.
/// The ordinary String policy preserves the existing diagnostic message.
pub trait SemanticReasonV3: Debug + From<&'static str> {
    /// Creates a semantic reason without changing the selected payload policy.
    fn custom_v3<U: Display>(msg: U) -> Self;
}

impl SemanticReasonV3 for String {
    fn custom_v3<U: Display>(msg: U) -> Self {
        msg.to_string()
    }
}

/// A finite semantic error for a caller that cannot retain a diagnostic String.
#[derive(Debug)]
pub enum FiniteSemanticReasonV3 {
    /// A parser-owned static diagnostic; no dynamic formatting is evaluated.
    Static(&'static str),
    /// A Serde custom diagnostic discarded before evaluating its formatter.
    CustomRefused,
}

impl From<&'static str> for FiniteSemanticReasonV3 {
    fn from(reason: &'static str) -> Self {
        Self::Static(reason)
    }
}

impl SemanticReasonV3 for FiniteSemanticReasonV3 {
    fn custom_v3<U: Display>(_msg: U) -> Self {
        // Never evaluate arbitrary Display on the native error path.
        Self::CustomRefused
    }
}

/// An error occurred during deserialization.
/// The default semantic payload retains the public ordinary error shape.
#[derive(Debug)]
pub enum Error<T, S = String> {
    /// An error occurred while reading bytes
    ///
    /// Contains the underlying error returned while reading.
    Io(T),

    /// An error occurred while parsing bytes
    ///
    /// Contains the offset into the stream where the syntax error occurred.
    Syntax(usize),

    /// An error occurred while processing a parsed value
    ///
    /// Contains a description of the error that occurred and (optionally)
    /// the offset into the stream indicating the start of the item being
    /// processed when the error occurred.
    Semantic(Option<usize>, S),

    /// The input caused serde to recurse too much
    ///
    /// This error prevents a stack overflow.
    RecursionLimitExceeded,
}

impl<T, S> Error<T, S> {
    /// A helper method for composing a semantic error
    #[inline]
    pub fn semantic(offset: impl Into<Option<usize>>, msg: impl Into<S>) -> Self {
        Self::Semantic(offset.into(), msg.into())
    }
}

impl<T, S> From<T> for Error<T, S> {
    #[inline]
    fn from(value: T) -> Self {
        Error::Io(value)
    }
}

impl<T, S> From<ciborium_ll::Error<T>> for Error<T, S> {
    #[inline]
    fn from(value: ciborium_ll::Error<T>) -> Self {
        match value {
            ciborium_ll::Error::Io(x) => Self::Io(x),
            ciborium_ll::Error::Syntax(x) => Self::Syntax(x),
        }
    }
}

impl<T: Debug, S: Debug> Display for Error<T, S> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "{self:?}")
    }
}

impl<T: Debug, S: Debug> StdError for Error<T, S> {}

impl<T: Debug, S: SemanticReasonV3> DeError for Error<T, S> {
    #[inline]
    fn custom<U: Display>(msg: U) -> Self {
        Self::Semantic(None, S::custom_v3(msg))
    }
}
