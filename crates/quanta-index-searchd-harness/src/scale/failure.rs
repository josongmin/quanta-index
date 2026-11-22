//! Typed scale admission, stage and cleanup failures shared by benchmark rails.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScaleAdmissionLimit {
    SourceFileBytes,
    TotalSourceBytes,
    TrigramPostingMemberships,
}

impl ScaleAdmissionLimit {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourceFileBytes => "lexical_source_file_bytes_8m",
            Self::TotalSourceBytes => "lexical_total_source_bytes_128m",
            Self::TrigramPostingMemberships => "lexical_trigram_posting_memberships",
        }
    }
}

#[derive(Debug)]
pub struct ScaleAdmissionRefusal {
    pub limit: ScaleAdmissionLimit,
    pub observed: u64,
    pub maximum: u64,
}

impl std::fmt::Display for ScaleAdmissionRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "scale: {} admission refused: observed {} > maximum {}",
            self.limit.as_str(),
            self.observed,
            self.maximum
        )
    }
}

impl std::error::Error for ScaleAdmissionRefusal {}

#[derive(Debug)]
pub struct ScaleStageError {
    pub stage: &'static str,
    pub limit: Option<ScaleAdmissionLimit>,
    pub observed: Option<u64>,
    pub maximum: Option<u64>,
    pub message: String,
}

impl std::fmt::Display for ScaleStageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "scale {}: {}", self.stage, self.message)
    }
}

impl std::error::Error for ScaleStageError {}

impl ScaleStageError {
    #[must_use]
    pub fn operation(stage: &'static str, error: &anyhow::Error) -> Self {
        Self {
            stage,
            limit: None,
            observed: None,
            maximum: None,
            message: format!("{error:#}"),
        }
    }

    #[must_use]
    pub fn source_admission(error: &anyhow::Error) -> Self {
        let refusal = error.downcast_ref::<ScaleAdmissionRefusal>();
        Self {
            stage: "source_preflight",
            limit: refusal.map(|refusal| refusal.limit),
            observed: refusal.map(|refusal| refusal.observed),
            maximum: refusal.map(|refusal| refusal.maximum),
            message: format!("{error:#}"),
        }
    }
}

/// Keep measurement and daemon teardown errors separate.
#[derive(Debug)]
pub(crate) struct ScaleRuntimeFailure {
    pub(crate) primary: Option<anyhow::Error>,
    pub(crate) cleanup: anyhow::Error,
    pub(crate) cleanup_context: &'static str,
}

impl std::fmt::Display for ScaleRuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(primary) = &self.primary {
            write!(
                formatter,
                "scale measurement failed: {primary:#}; {} failed: {:#}",
                self.cleanup_context, self.cleanup
            )
        } else {
            write!(
                formatter,
                "scale {} failed: {:#}",
                self.cleanup_context, self.cleanup
            )
        }
    }
}

impl std::error::Error for ScaleRuntimeFailure {}

pub(crate) fn stage_or_preserve<E: Into<anyhow::Error>>(
    stage: &'static str,
    error: E,
) -> anyhow::Error {
    let error = error.into();
    if error.downcast_ref::<ScaleStageError>().is_some()
        || error.downcast_ref::<ScaleRuntimeFailure>().is_some()
    {
        error
    } else {
        ScaleStageError::operation(stage, &error).into()
    }
}
