//! Native text match enumeration under the `NativeTextMatchesRequest`
//! contract (ticket 10.1b engine core).
//!
//! Each admitted Source of the selected view is split into lines at `\n`
//! (the terminator is not part of a line; `\r` stays in it, as in ripgrep).
//! Every line is searched by the configured document engine
//! (`quanta_index_lq_regex::engine_config`, UTF-8 empty matches) with the
//! iteration of `regex::Regex::find_iter`: matches are leftmost-first and
//! non-overlapping, and empty matches are dropped. The engine holds a grant
//! for its reported heap plus the configured bound of its one search cache.
//! Lines are valid UTF-8 and patterns that could match invalid UTF-8 are
//! refused, so this equals the contract's `regex::bytes` formulation and
//! every range is UTF-8 aligned. A literal is the regex of its escaped text. Patterns
//! that need a literal `\n` are refused.
//!
//! Candidate files are admitted before any body is read. A query whose
//! required literals anchor on the pinned Literal sibling reads only the
//! anchored owners: a literal needle is required as written, byte-exactly or
//! under simple case folding, and a regex requires the prefix or suffix
//! literals extracted from its HIR (see `text_required_literals`). Matching
//! per line only removes matches, so a body-level anchor stays a superset.
//! Every other query uses the bounded member census. Either source holds at most
//! `MAX_UNANCHORED_DOCS` files, otherwise the request is refused with
//! `ResourceLimited` and no body is read. Every body read is charged, and a
//! budget or cancellation refusal refuses the whole page.

use super::text::{BoundTextMember, NativeTextIndex, TextReadContext};
use super::text_filter::resolve_for_surface;
use super::text_literal_anchor::pinned_literal_sibling;
use super::text_regex::{admit_parse, document_construction_bytes};
use super::text_required_literals::{RequiredAnchor, RequiredLiterals, anchor_required};
use super::text_universe::{CandidateUniverse, MAX_UNANCHORED_DOCS};
use quanta_index_contract::native_protocol::{
    NativeTextMatch, NativeTextMatchCase, NativeTextMatchCompletion, NativeTextMatchCursor,
    NativeTextMatchPage, NativeTextMatchPattern, NativeTextMatchQuery, NativeTextMatchesRequest,
    native_text_match_query_digest,
};
use quanta_index_contract::{NativeSourceIdentity, PreviewByteRange, RepoId, index::ViewSelection};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, SourceRecordOwner, WorkBudget,
    WorkUsage, owner_map_key,
};
use regex_automata::util::iter::Searcher;
use regex_automata::{Input, Match, meta};
use regex_syntax::hir::{Class, Hir, HirKind};

/// One compiled match query.
pub(super) struct MatchPattern {
    regex: meta::Regex,
    /// The one search cache every line search reuses.
    cache: meta::Cache,
    /// Independently sound required-literal sets, tried in order.
    required: Vec<RequiredLiterals>,
    // The engine and its cache die before the grant for their retained heap.
    _memory: Option<MemoryLease>,
}

/// Compile one query under the contract's semantics, or refuse it.
pub(super) fn compile_match_pattern(
    query: &NativeTextMatchQuery,
    budget: &mut WorkBudget,
) -> Result<MatchPattern, IndexError> {
    query
        .validate()
        .map_err(|message| IndexError::InvalidInput(message.into()))?;
    let (pattern, literal) = match &query.pattern {
        NativeTextMatchPattern::Literal(text) => {
            if text.contains('\n') {
                return Err(IndexError::InvalidInput(
                    "native text match literal needs a line terminator".into(),
                ));
            }
            (regex::escape(text), Some(text.as_str()))
        }
        NativeTextMatchPattern::Regex(text) => (text.clone(), None),
    };
    let insensitive = query.case == NativeTextMatchCase::Insensitive;
    // The parser and the HIR it returns are admitted before the parser runs
    // and held until the compilation ends.
    let (_pattern_bytes, parse_memory) = admit_parse(pattern.len(), budget)?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(pattern.len())
            .map_err(|_width| IndexError::ResourceLimited)?
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    // UTF-8 mode refuses every construct that could match a partial scalar.
    let hir = regex_syntax::ParserBuilder::new()
        .case_insensitive(insensitive)
        .utf8(true)
        .unicode(true)
        .build()
        .parse(&pattern)
        .map_err(|error| IndexError::InvalidInput(format!("native text match regex: {error}")))?;
    if requires_line_terminator(&hir, budget)? {
        return Err(IndexError::InvalidInput(
            "native text match pattern needs a line terminator".into(),
        ));
    }
    // The engine is the configured document engine: its NFAs under
    // construction are admitted until it exists, and it then holds its
    // reported heap plus the configured bound of its one search cache.
    let construction = document_construction_bytes()?;
    let construction_memory = budget.reserve_memory(construction, MemoryLifetime::Temporary)?;
    budget.checkpoint()?;
    let regex = meta::Regex::builder()
        .configure(quanta_index_lq_regex::engine_config().utf8_empty(true))
        .build_from_hir(&hir)
        .map_err(|error| {
            if error.size_limit().is_some() {
                IndexError::ResourceLimited
            } else {
                IndexError::InvalidInput(format!("native text match regex: {error}"))
            }
        })?;
    let cache = regex.create_cache();
    let retained = quanta_index_lq_regex::engine_retained_bytes(regex.memory_usage())
        .ok_or(IndexError::ResourceLimited)?;
    let retained = u64::try_from(retained).map_err(|_width| IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(retained, MemoryLifetime::Temporary)?;
    drop(construction_memory);
    // Every match of an escaped literal is one raw expansion of the needle:
    // each character matches itself, or its simple case folding class. A
    // regex is anchored by the literals its compiled HIR requires.
    let required = match literal {
        Some(text) => vec![RequiredLiterals::literal(text, insensitive)],
        None => RequiredLiterals::of_regex(&hir, insensitive, budget)?,
    };
    drop(hir);
    drop(parse_memory);
    Ok(MatchPattern {
        regex,
        cache,
        required,
        _memory: memory,
    })
}

impl MatchPattern {
    /// The next leftmost-first match of a line searcher, empty or not. This
    /// is the iteration of `regex::Regex::find_iter`: an empty match never
    /// overlaps the previous match and never splits a UTF-8 scalar.
    fn next_match(&mut self, searcher: &mut Searcher<'_>) -> Result<Option<Match>, IndexError> {
        let Self { regex, cache, .. } = self;
        searcher
            .try_advance(|input| Ok(regex.search_with(cache, input)))
            .map_err(|error| {
                IndexError::BackendViolation(format!("native text match search: {error}"))
            })
    }
}

#[cfg(test)]
impl MatchPattern {
    /// The engine's reported heap, its search cache's heap, and the bytes of
    /// the grant this pattern holds.
    #[expect(
        clippy::used_underscore_binding,
        reason = "the grant is held only for its drop outside tests"
    )]
    pub(super) fn heap_for_test(&self) -> (usize, usize, u64) {
        (
            self.regex.memory_usage(),
            self.cache.memory_usage(),
            self._memory.as_ref().map_or(0, MemoryLease::bytes),
        )
    }

    /// Every match of one line, empty ones included, as the scan finds them.
    pub(super) fn line_matches_for_test(
        &mut self,
        line: &str,
    ) -> Result<Vec<(usize, usize)>, IndexError> {
        let mut searcher = Searcher::new(Input::new(line));
        let mut found = Vec::new();
        while let Some(next) = self.next_match(&mut searcher)? {
            found.push((next.start(), next.end()));
        }
        Ok(found)
    }
}

/// Whether every nonempty match needs a line terminator, which no line
/// contains. An optional or alternative newline must not hide a valid match.
fn requires_line_terminator(hir: &Hir, budget: &WorkBudget) -> Result<bool, IndexError> {
    #[derive(Clone, Copy)]
    struct LineFree {
        empty: bool,
        nonempty: bool,
        has_newline: bool,
    }

    fn inspect(node: &Hir, budget: &WorkBudget) -> Result<LineFree, IndexError> {
        budget.checkpoint()?;
        let result = match node.kind() {
            HirKind::Empty | HirKind::Look(_) => LineFree {
                empty: true,
                nonempty: false,
                has_newline: false,
            },
            HirKind::Literal(literal) => {
                let newline = literal.0.contains(&b'\n');
                LineFree {
                    empty: literal.0.is_empty(),
                    nonempty: !newline && !literal.0.is_empty(),
                    has_newline: newline,
                }
            }
            HirKind::Class(Class::Unicode(class)) => {
                let mut nonempty = false;
                let mut has_newline = false;
                for range in class.ranges() {
                    let newline = range.start() <= '\n' && '\n' <= range.end();
                    has_newline |= newline;
                    nonempty |= !newline || range.start() != range.end();
                }
                LineFree {
                    empty: false,
                    nonempty,
                    has_newline,
                }
            }
            HirKind::Class(Class::Bytes(class)) => {
                let mut nonempty = false;
                let mut has_newline = false;
                for range in class.ranges() {
                    let newline = range.start() <= b'\n' && b'\n' <= range.end();
                    has_newline |= newline;
                    nonempty |= !newline || range.start() != range.end();
                }
                LineFree {
                    empty: false,
                    nonempty,
                    has_newline,
                }
            }
            HirKind::Repetition(repetition) => {
                let sub = inspect(&repetition.sub, budget)?;
                if repetition.min == 0 {
                    LineFree {
                        empty: true,
                        nonempty: repetition.max != Some(0) && sub.nonempty,
                        has_newline: repetition.max != Some(0) && sub.has_newline,
                    }
                } else {
                    sub
                }
            }
            HirKind::Capture(capture) => inspect(&capture.sub, budget)?,
            HirKind::Alternation(children) => {
                let mut result = LineFree {
                    empty: false,
                    nonempty: false,
                    has_newline: false,
                };
                for child in children {
                    let next = inspect(child, budget)?;
                    result.empty |= next.empty;
                    result.nonempty |= next.nonempty;
                    result.has_newline |= next.has_newline;
                }
                result
            }
            HirKind::Concat(children) => {
                let mut result = LineFree {
                    empty: true,
                    nonempty: false,
                    has_newline: false,
                };
                for child in children {
                    let next = inspect(child, budget)?;
                    result.nonempty = (result.nonempty && (next.empty || next.nonempty))
                        || (result.empty && next.nonempty);
                    result.empty &= next.empty;
                    result.has_newline |= next.has_newline;
                }
                result
            }
        };
        Ok(result)
    }

    let result = inspect(hir, budget)?;
    Ok(result.has_newline && !result.nonempty)
}

/// One admitted candidate file of the selected view.
struct MatchFile {
    repository: Option<RepoId>,
    path: String,
    owner: OwnerKey,
    source: ObjectRef,
    body: ObjectRef,
    byte_len: u64,
}

/// A match page with the retained grants of its owned rows.
pub(super) struct ScannedTextMatches {
    pub(super) page: NativeTextMatchPage,
    // Rows die before the grants that admitted their allocations.
    _memory: Vec<MemoryLease>,
}

/// Enumerate one page of matches of `request` in the selected view.
pub(super) fn scan_text_matches(
    index: &NativeTextIndex,
    bound: &BoundTextMember,
    request: &NativeTextMatchesRequest,
    context: &mut TextReadContext<'_>,
) -> Result<ScannedTextMatches, IndexError> {
    request.validate().map_err(IndexError::InvalidInput)?;
    let view = context.ticket.view.identity;
    let selected = match request.selection {
        ViewSelection::Exact(requested) => requested == view,
        ViewSelection::Latest { workspace, epoch } => {
            workspace == view.workspace && epoch == view.epoch
        }
    };
    if !selected || bound.member().instance != request.instance {
        return Err(IndexError::Conflict);
    }
    let query = native_text_match_query_digest(request.instance, &request.query)
        .map_err(IndexError::BackendViolation)?;
    let mut pattern = compile_match_pattern(&request.query, context.budget)?;
    let (files, _files_memory) = candidate_files(index, bound, &pattern, request, context)?;
    let limit = usize::try_from(request.limit).map_err(|_width| IndexError::ResourceLimited)?;
    let mut memory = Vec::new();
    memory
        .try_reserve_exact(limit.checked_add(1).ok_or(IndexError::ResourceLimited)?)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let rows_bytes = limit
        .checked_mul(std::mem::size_of::<NativeTextMatch>())
        .ok_or(IndexError::ResourceLimited)?;
    if let Some(lease) = context.budget.reserve_memory(
        u64::try_from(rows_bytes).map_err(|_width| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )? {
        memory.push(lease);
    }
    let mut rows: Vec<NativeTextMatch> = Vec::new();
    rows.try_reserve_exact(limit)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let after = request.after.as_ref();
    let mut more = false;
    'files: for file in &files {
        let order = after.map(|cursor| {
            (&file.repository, file.path.as_str()).cmp(&(&cursor.repository, cursor.path.as_str()))
        });
        let skip_through = match (after, order) {
            (Some(_), Some(std::cmp::Ordering::Less)) => continue,
            (Some(cursor), Some(std::cmp::Ordering::Equal)) => {
                // The cursor's file must still be the same Source record,
                // owner and Source both, in this view.
                if cursor.native_source
                    != (NativeSourceIdentity {
                        owner: file.owner,
                        source: file.source,
                    })
                {
                    return Err(IndexError::Conflict);
                }
                Some(cursor.start)
            }
            _ => None,
        };
        let body = context.objects.get(file.body, context.budget)?;
        context.budget.charge(WorkUsage {
            reads: 1,
            bytes: file.byte_len,
            ..WorkUsage::default()
        })?;
        if body.registration.reference != file.body
            || !body.registration.children.is_empty()
            || u64::try_from(body.payload.len()).map_err(|_width| IndexError::ResourceLimited)?
                != file.byte_len
        {
            return Err(IndexError::CorruptObject(
                "native text match source bytes differ from the admitted leaf".into(),
            ));
        }
        let text = std::str::from_utf8(&body.payload).map_err(|_utf8| {
            IndexError::CorruptObject("native text match admitted source is not UTF-8".into())
        })?;
        let mut offset = 0_u64;
        for line in text.split('\n') {
            context.budget.checkpoint()?;
            let mut searcher = Searcher::new(Input::new(line));
            while let Some(found) = pattern.next_match(&mut searcher)? {
                if found.start() == found.end() {
                    continue;
                }
                let start = offset
                    .checked_add(
                        u64::try_from(found.start())
                            .map_err(|_width| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                let end = offset
                    .checked_add(
                        u64::try_from(found.end()).map_err(|_width| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                if skip_through.is_some_and(|through| start <= through) {
                    continue;
                }
                if rows.len() == limit {
                    more = true;
                    break 'files;
                }
                let bytes = file
                    .path
                    .len()
                    .checked_add(file.repository.as_ref().map_or(0, RepoId::backing_capacity))
                    .ok_or(IndexError::ResourceLimited)?;
                if let Some(lease) = context.budget.reserve_memory(
                    u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
                    MemoryLifetime::Retained,
                )? {
                    memory.push(lease);
                }
                rows.push(NativeTextMatch {
                    repository: file.repository.clone(),
                    path: file.path.clone(),
                    native_source: NativeSourceIdentity {
                        owner: file.owner,
                        source: file.source,
                    },
                    range: PreviewByteRange { start, end },
                });
            }
            offset = offset
                .checked_add(
                    u64::try_from(line.len()).map_err(|_width| IndexError::ResourceLimited)?,
                )
                .and_then(|value| value.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?;
        }
    }
    let (completion, next) = if more {
        let last = rows.last().ok_or_else(|| {
            IndexError::BackendViolation("native text match continuation has no row".into())
        })?;
        (
            NativeTextMatchCompletion::More,
            Some(NativeTextMatchCursor {
                view,
                instance: request.instance,
                query,
                repository: last.repository.clone(),
                path: last.path.clone(),
                native_source: last.native_source,
                start: last.range.start,
            }),
        )
    } else {
        (NativeTextMatchCompletion::Exhausted, None)
    };
    let page = NativeTextMatchPage {
        view,
        instance: request.instance,
        query,
        matches: rows,
        completion,
        next,
    };
    page.validate_for(request).map_err(|message| {
        IndexError::BackendViolation(format!("native text match page: {message}"))
    })?;
    context.budget.checkpoint()?;
    Ok(ScannedTextMatches {
        page,
        _memory: memory,
    })
}

/// Admit every candidate file and read its Source record, never its body.
/// The result is ordered by `(repository, path)` and holds one file per key.
fn candidate_files(
    index: &NativeTextIndex,
    bound: &BoundTextMember,
    pattern: &MatchPattern,
    request: &NativeTextMatchesRequest,
    context: &mut TextReadContext<'_>,
) -> Result<(Vec<MatchFile>, Vec<MemoryLease>), IndexError> {
    let selection = resolve_for_surface(index, &request.query.constraints, false, context)?;
    let root = bound.root(index, context)?;
    let recipe = index.recipe();
    let mut anchor: Option<RequiredAnchor> = None;
    if let (None, false, Some(format)) = (
        selection.selected_len(),
        pattern.required.is_empty(),
        index.literal_sibling(),
    ) && let Some(grams) = pinned_literal_sibling(format, bound.member(), context)?
    {
        for required in &pattern.required {
            anchor = anchor_required(grams, required, context)?;
            if anchor.is_some() {
                break;
            }
        }
    }
    let mut universe = match (&anchor, selection.selected_len()) {
        (Some(anchor), None) => CandidateUniverse::anchored(
            root,
            recipe,
            std::iter::once(anchor.owners()),
            context.budget,
        )?,
        // A path selection is bounded by its own certified length; the full
        // census refuses more than `MAX_UNANCHORED_DOCS` admitted documents.
        (_, Some(_)) => CandidateUniverse::new(root, recipe, &selection, true)?,
        (None, None) => CandidateUniverse::new(root, recipe, &selection, false)?,
    };
    let mut files = Vec::new();
    let mut memory = Vec::new();
    while let Some(candidate) = universe.next(context)? {
        let admitted = u64::try_from(files.len()).map_err(|_width| IndexError::ResourceLimited)?;
        if admitted >= MAX_UNANCHORED_DOCS {
            return Err(IndexError::ResourceLimited);
        }
        let file = admitted_file(index, candidate.owner, candidate.source_record, context)?;
        let bytes = file
            .path
            .len()
            .checked_add(file.repository.as_ref().map_or(0, RepoId::backing_capacity))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MatchFile>()))
            .ok_or(IndexError::ResourceLimited)?;
        if let Some(lease) = context.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )? {
            memory.push(lease);
        }
        files
            .try_reserve(1)
            .map_err(|_error| IndexError::ResourceLimited)?;
        files.push(file);
    }
    drop(anchor);
    files.sort_by(|left, right| {
        (&left.repository, left.path.as_str()).cmp(&(&right.repository, right.path.as_str()))
    });
    if files
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if left.repository == right.repository && left.path == right.path))
    {
        return Err(IndexError::CorruptObject(
            "two selected owners claim one repository path".into(),
        ));
    }
    Ok((files, memory))
}

/// The authenticated Source record of one admitted candidate owner.
fn admitted_file(
    index: &NativeTextIndex,
    owner: OwnerKey,
    source: ObjectRef,
    context: &mut TextReadContext<'_>,
) -> Result<MatchFile, IndexError> {
    context.budget.checkpoint()?;
    if context.maps.lookup_object(
        context.ticket.view.source.owners,
        &owner_map_key(owner),
        context.budget,
    )? != Some(source)
    {
        return Err(IndexError::CorruptObject(
            "native text match owner differs from the selected source".into(),
        ));
    }
    let stored = context.objects.get(source, context.budget)?;
    let record =
        SourceRecordOwner::decode_json(&stored.payload, context.budget, MemoryLifetime::Temporary)?;
    if stored.registration.reference != source
        || stored.registration.children.as_slice() != [record.bytes]
        || !index.validate_selected_source(&record)?
        || !quanta_index_contract::index::is_canonical_source_path(&record.path)
    {
        return Err(IndexError::CorruptObject(
            "native text match candidate differs from its admitted source".into(),
        ));
    }
    Ok(MatchFile {
        repository: record.source_repo_id.clone(),
        path: record.path.clone(),
        owner,
        source,
        body: record.bytes,
        byte_len: record.byte_len,
    })
}
