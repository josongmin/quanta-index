use std::collections::BTreeMap;
use std::fmt;

use crate::corpus::ExpectedShape;
use crate::errors::ConformanceError;
use crate::runner::{CandidateShape, ConformanceExecutor, LqQueryNormalizer};

/// Canned executor response keyed by query string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MockResponse {
    /// Executor returns this candidate shape.
    Ok(CandidateShape),
    /// Executor returns this typed error code.
    Err(ConformanceError),
}

/// Thin error wrapper carrying a `ConformanceError` payload —
/// `LqQueryNormalizer::Error` and `ConformanceExecutor::Error` must
/// both implement `core::error::Error`, so we wrap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MockError(pub ConformanceError);

impl fmt::Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl core::error::Error for MockError {}

/// Mock normalizer.
///
/// By default `parse_and_normalize` returns the query echoed back
/// as the normalized form, and `canonical_hash` returns a fixed
/// 32-byte array. `fail_on` configures failure for a specific input.
#[derive(Clone, Debug, Default)]
pub struct MockNormalizer {
    fixed_hash: [u8; 32],
    failures: BTreeMap<String, ConformanceError>,
}

impl MockNormalizer {
    /// Configure the normalizer to fail with `code` when given
    /// exactly `input`.
    #[must_use]
    pub fn fail_on(mut self, input: &str, code: ConformanceError) -> Self {
        // `insert` returns the prior entry — we intentionally discard
        // it. Using `let _: Option<_> = ...` would trip
        // `let_underscore_must_use`; binding to a typed sink keeps it
        // explicit without invoking `drop` on a non-`Drop` payload.
        let _prior: Option<ConformanceError> = self.failures.insert(input.to_owned(), code);
        self
    }

    /// Set the canned canonical hash.
    #[must_use]
    pub const fn with_hash(mut self, hash: [u8; 32]) -> Self {
        self.fixed_hash = hash;
        self
    }
}

impl LqQueryNormalizer for MockNormalizer {
    type Normalized = String;
    type Error = MockError;

    fn parse_and_normalize(&self, input: &str) -> Result<Self::Normalized, Self::Error> {
        if let Some(code) = self.failures.get(input) {
            return Err(MockError(code.clone()));
        }
        Ok(input.to_owned())
    }

    fn canonical_hash(&self, _q: &Self::Normalized) -> [u8; 32] {
        self.fixed_hash
    }

    fn classify(&self, err: &Self::Error) -> ConformanceError {
        err.0.clone()
    }
}

/// Mock executor.
///
/// Lookup is keyed by the normalized string (which `MockNormalizer`
/// produces by echoing the input). Missing keys produce a
/// `ConformanceError::NotImplemented` error.
#[derive(Clone, Debug, Default)]
pub struct MockExecutor {
    responses: BTreeMap<String, MockResponse>,
}

impl MockExecutor {
    /// Empty executor — every query returns `NotImplemented`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a canned response for `query`.
    #[must_use]
    pub fn with(mut self, query: &str, response: MockResponse) -> Self {
        let _prior: Option<MockResponse> = self.responses.insert(query.to_owned(), response);
        self
    }
}

impl ConformanceExecutor<String> for MockExecutor {
    type Error = MockError;

    fn execute(
        &self,
        normalized: &String,
        _expected: &ExpectedShape,
    ) -> Result<CandidateShape, Self::Error> {
        match self.responses.get(normalized) {
            Some(MockResponse::Ok(shape)) => Ok(shape.clone()),
            Some(MockResponse::Err(code)) => Err(MockError(code.clone())),
            None => Err(MockError(ConformanceError::NotImplemented)),
        }
    }

    fn classify(&self, err: &Self::Error) -> ConformanceError {
        err.0.clone()
    }
}
