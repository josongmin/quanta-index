//! Bounded Native regex verification over authenticated source bytes.

use crate::{
    filters::{FilePathScope, PathConstraint},
    normalize,
};
use quanta_index_core::index::{
    IndexError, MAX_SOURCE_PATH_BYTES, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey,
    SourceRecordOwner, WorkBudget, WorkUsage, owner_map_key,
};
use quanta_index_lq_regex::{RegexCompilationPlan, RegexError, RegexErrorCode, RegexExecutor};
use quanta_index_lq_text_normalizer::CaseMode;

use super::text::{NativeTextIndex, TextReadContext};
use super::text_overlay::MAX_TEXT_OVERLAY_DOCUMENT_BYTES;

pub(super) const MAX_NATIVE_CONTENT_REGEX: usize = 4;
pub(super) const MAX_NATIVE_PATH_FILTERS: usize = 4;
pub(super) const MAX_NATIVE_REGEX_PATTERN_BYTES: usize = 4_096;

// The executor config caps one NFA at 10 MiB and one DFA cache at 2 MiB.
// This is a logical admission charge per engine, not an aggregate RSS bound.
const ENGINE_LOGICAL_BYTES: u64 = 12 * 1024 * 1024;
// Parser/HIR is admitted before RegexExecutor::prepare and retained while the
// executor owns its HIR. The state estimator is structural, so this is a
// logical policy, not heap proof.
const PREPARE_LOGICAL_BYTES: u64 = quanta_index_lq_regex::MAX_NFA_STATES * 32;

struct ContentRegex {
    executor: RegexExecutor,
    estimated_states: u64,
}

struct PathRegex {
    executor: RegexExecutor,
    scope: FilePathScope,
    estimated_states: u64,
}

// RegexExecutor keeps the pattern and HIR after compilation. Keep their
// admission leases alive for the same lifetime as the compiled automaton.
struct RegexMemory {
    _pattern: Option<MemoryLease>,
    _prepared: Option<MemoryLease>,
    _engine: Option<MemoryLease>,
}

pub(super) struct NativeRegexProgram {
    content: Vec<ContentRegex>,
    paths: Vec<PathRegex>,
    engine_memory: Vec<RegexMemory>,
    _container_memory: Option<MemoryLease>,
}

fn map_regex_error(error: &RegexError) -> IndexError {
    if error.code == RegexErrorCode::PlanLimitExceeded {
        IndexError::ResourceLimited
    } else {
        IndexError::InvalidInput(format!("native regex: {error}"))
    }
}

fn compile_one(
    source: &str,
    case: Option<CaseMode>,
    budget: &mut WorkBudget,
) -> Result<(RegexExecutor, u64, RegexMemory), IndexError> {
    if source.len() > MAX_NATIVE_REGEX_PATTERN_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let pattern_bytes = u64::try_from(source.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: pattern_bytes
            .checked_add(PREPARE_LOGICAL_BYTES)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let pattern_memory = budget.reserve_memory(pattern_bytes, MemoryLifetime::Temporary)?;
    let prepare_memory = budget.reserve_memory(PREPARE_LOGICAL_BYTES, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    let pattern = match case {
        Some(CaseMode::Folded) => format!("(?i){}", normalize::nfc(source)),
        Some(CaseMode::Sensitive) => normalize::nfc(source).into_owned(),
        None => source.to_owned(),
    };
    let prepared: RegexCompilationPlan =
        RegexExecutor::prepare(&pattern).map_err(|error| map_regex_error(&error))?;
    let states = prepared.estimated_states();
    budget.charge(WorkUsage {
        bytes: states
            .checked_add(ENGINE_LOGICAL_BYTES)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let engine_memory = budget.reserve_memory(ENGINE_LOGICAL_BYTES, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    let executor =
        RegexExecutor::compile_prepared(prepared).map_err(|error| map_regex_error(&error))?;
    budget.checkpoint()?;
    Ok((
        executor,
        states.max(1),
        RegexMemory {
            _pattern: pattern_memory,
            _prepared: prepare_memory,
            _engine: engine_memory,
        },
    ))
}

impl NativeRegexProgram {
    pub(super) fn compile(
        content: &[&str],
        paths: &[PathConstraint],
        case: CaseMode,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        if content.len() > MAX_NATIVE_CONTENT_REGEX || paths.len() > MAX_NATIVE_PATH_FILTERS {
            return Err(IndexError::ResourceLimited);
        }
        let count = content
            .len()
            .checked_add(paths.len())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = content
            .len()
            .checked_mul(std::mem::size_of::<ContentRegex>())
            .and_then(|bytes| {
                paths
                    .len()
                    .checked_mul(std::mem::size_of::<PathRegex>())
                    .and_then(|path| bytes.checked_add(path))
            })
            .and_then(|bytes| {
                count
                    .checked_mul(std::mem::size_of::<RegexMemory>())
                    .and_then(|leases| bytes.checked_add(leases))
            })
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let container_memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Temporary)?
        };
        let mut program = Self {
            content: Vec::new(),
            paths: Vec::new(),
            engine_memory: Vec::new(),
            _container_memory: container_memory,
        };
        program
            .content
            .try_reserve_exact(content.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        program
            .paths
            .try_reserve_exact(paths.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        program
            .engine_memory
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for source in content {
            let (executor, estimated_states, memory) = compile_one(source, Some(case), budget)?;
            program.content.push(ContentRegex {
                executor,
                estimated_states,
            });
            program.engine_memory.push(memory);
        }
        for path in paths {
            let (executor, estimated_states, memory) = compile_one(&path.pattern, None, budget)?;
            program.paths.push(PathRegex {
                executor,
                scope: path.scope,
                estimated_states,
            });
            program.engine_memory.push(memory);
        }
        Ok(program)
    }

    pub(super) fn has_content(&self) -> bool {
        !self.content.is_empty()
    }

    fn matches_path(&self, path: &str, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        let file_name = path
            .rsplit('/')
            .next()
            .ok_or_else(|| IndexError::CorruptObject("native path lost file name".into()))?;
        let scan_len = path
            .len()
            .checked_add(file_name.len())
            .ok_or(IndexError::ResourceLimited)?;
        let scan_len = u64::try_from(scan_len).map_err(|_error| IndexError::ResourceLimited)?;
        let bytes = self.paths.iter().try_fold(0u64, |total, filter| {
            scan_len
                .checked_mul(filter.estimated_states)
                .and_then(|work| total.checked_add(work))
                .ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let mut matched = true;
        for filter in &self.paths {
            budget.checkpoint()?;
            let path_match = filter.executor.verify(path.as_bytes());
            let name_match = filter.executor.verify(file_name.as_bytes());
            matched &= match filter.scope {
                FilePathScope::NameAndPath => path_match || name_match,
                FilePathScope::NameOnly => name_match,
                FilePathScope::PathOnly => path_match,
            };
        }
        budget.checkpoint()?;
        Ok(matched)
    }

    fn evaluate_text(
        &self,
        source_text: &str,
        path_match: bool,
        budget: &mut WorkBudget,
    ) -> Result<(u8, bool), IndexError> {
        if self.content.is_empty() {
            return Ok((0, path_match));
        }
        let source_len =
            u64::try_from(source_text.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let normalization_bytes = source_len
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?;
        let _normalized_memory = if normalization_bytes == 0 {
            None
        } else {
            budget.reserve_memory(normalization_bytes, MemoryLifetime::Temporary)?
        };
        budget.checkpoint()?;
        let normalized = normalize::nfc(source_text);
        let normalized_len =
            u64::try_from(normalized.len()).map_err(|_error| IndexError::ResourceLimited)?;
        if normalized_len > normalization_bytes {
            return Err(IndexError::ResourceLimited);
        }
        let scan_bytes = self.content.iter().try_fold(0u64, |total, filter| {
            normalized_len
                .max(1)
                .checked_mul(filter.estimated_states)
                .and_then(|work| total.checked_add(work))
                .ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            bytes: scan_bytes,
            ..WorkUsage::default()
        })?;
        let mut mask = 0u8;
        for (ordinal, filter) in self.content.iter().enumerate() {
            budget.checkpoint()?;
            if filter.executor.verify(normalized.as_bytes()) {
                mask |= 1u8
                    .checked_shl(
                        u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        budget.checkpoint()?;
        Ok((mask, path_match))
    }

    pub(super) fn evaluate_overlay(
        &self,
        path: &str,
        body: &str,
        budget: &mut WorkBudget,
    ) -> Result<(u8, bool), IndexError> {
        budget.checkpoint()?;
        if path.is_empty()
            || path.len() > MAX_SOURCE_PATH_BYTES
            || path.starts_with('/')
            || path.contains('\0')
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || body.len() > MAX_TEXT_OVERLAY_DOCUMENT_BYTES
        {
            return Err(IndexError::InvalidInput(
                "native regex overlay source exceeds admitted shape".into(),
            ));
        }
        let path_match = self.matches_path(path, budget)?;
        self.evaluate_text(body, path_match, budget)
    }

    /// The posting or admitted-universe cursor established `source`; recheck
    /// its selected owner and immutable edges before inspecting path or bytes.
    pub(super) fn evaluate(
        &self,
        index: &NativeTextIndex,
        owner: OwnerKey,
        source: ObjectRef,
        context: &mut TextReadContext<'_>,
    ) -> Result<(u8, bool), IndexError> {
        context.budget.checkpoint()?;
        if context.maps.lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner),
            context.budget,
        )? != Some(source)
        {
            return Err(IndexError::CorruptObject(
                "native regex source owner differs from selected view".into(),
            ));
        }
        let stored = context.objects.get(source, context.budget)?;
        let record = SourceRecordOwner::decode_json(
            &stored.payload,
            context.budget,
            MemoryLifetime::Temporary,
        )?;
        if stored.registration.reference != source
            || stored.registration.children.as_slice() != [record.bytes]
            || !index.validate_selected_source(&record)?
            || record.path.is_empty()
            || record.path.starts_with('/')
            || record.path.contains('\0')
            || record
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(IndexError::CorruptObject(
                "native regex source record differs from admitted selection".into(),
            ));
        }
        let path_match = self.matches_path(&record.path, context.budget)?;
        if self.content.is_empty() {
            return Ok((0, path_match));
        }
        let body = context.objects.get(record.bytes, context.budget)?;
        if body.registration.reference != record.bytes
            || !body.registration.children.is_empty()
            || u64::try_from(body.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
                != record.byte_len
        {
            return Err(IndexError::CorruptObject(
                "native regex source bytes differ from admitted leaf".into(),
            ));
        }
        let source_text = std::str::from_utf8(&body.payload).map_err(|_error| {
            IndexError::CorruptObject(
                "native regex admitted UTF-8 source contains invalid bytes".into(),
            )
        })?;
        self.evaluate_text(source_text, path_match, context.budget)
    }
}

#[cfg(test)]
#[path = "text_regex/tests.rs"]
mod tests;
