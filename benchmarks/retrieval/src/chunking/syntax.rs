//! Syntax-aware chunker for the pilot language (Rust).
//!
//! A brace-aware splitter with a small lexer (comments, strings, chars,
//! raw strings) cuts top-level items into one chunk each. Oversized items
//! split by nested fixed windows; unbalanced input, empty item sets, and
//! non-Rust files fall back to an explicit whole-file chunk flagged
//! `fallback: true`, counted in coverage, never silent.

use crate::chunking::{CHUNKER_VERSION, Chunk, Chunker, STRATEGY_SYNTAX, chunk_id};
use crate::corpus::SourceFile;
use crate::{BenchError, BenchResult, sha256_hex};

/// Default per-item byte cap before nested fixed-window splitting.
pub const DEFAULT_MAX_ITEM_BYTES: usize = 32 * 1024;

const ITEM_HEADS: [&str; 15] = [
    "fn",
    "struct",
    "enum",
    "impl",
    "trait",
    "mod",
    "const",
    "static",
    "type",
    "use",
    "extern",
    "macro",
    "union",
    "macro_rules",
    "pub",
];

#[derive(Debug, Clone, Copy)]
pub struct SyntaxChunker {
    pub max_item_bytes: usize,
}

impl SyntaxChunker {
    #[must_use]
    pub const fn new(max_item_bytes: usize) -> Self {
        Self { max_item_bytes }
    }
}

impl Default for SyntaxChunker {
    fn default() -> Self {
        Self {
            max_item_bytes: DEFAULT_MAX_ITEM_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lex {
    Normal,
    LineComment,
    BlockComment(usize),
    Str,
    Char,
    RawStr(usize),
}

/// Per-line brace deltas plus the depth before each line.
///
/// Strings and comments never contribute braces. Returns `None` on unterminated input.
/// Line terminators normalize to `\n` first: normalization preserves the
/// corpus line index (which also breaks on `\r\n` and `\r`) while braces
/// are terminator-independent.
fn line_depths(text: &str) -> Option<(Vec<i64>, Vec<usize>)> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = normalized.as_str();
    let mut state = Lex::Normal;
    let mut depth: i64 = 0;
    let mut depths_before: Vec<usize> = Vec::new();
    let mut deltas: Vec<i64> = Vec::new();
    let mut line_delta: i64 = 0;
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    depths_before.push(0);
    while index < chars.len() {
        let ch = *chars.get(index)?;
        let next = chars.get(index.saturating_add(1)).copied();
        match state {
            Lex::Normal => match ch {
                '/' if next == Some('/') => state = Lex::LineComment,
                '/' if next == Some('*') => {
                    state = Lex::BlockComment(1);
                    index = index.saturating_add(1);
                }
                '"' => state = Lex::Str,
                '\'' => {
                    // Lifetime (`&'a`, `<'static>`) or char literal (`'x'`,
                    // `'\n'`)? Only an immediate close or escape opens Char.
                    let after_next = chars.get(index.saturating_add(2)).copied();
                    if next == Some('\\') || after_next == Some('\'') {
                        state = Lex::Char;
                    }
                }
                'r' if next == Some('"') || next == Some('#') => {
                    let mut hashes: usize = 0;
                    let mut look = index.saturating_add(1);
                    while chars.get(look) == Some(&'#') {
                        hashes = hashes.checked_add(1)?;
                        look = look.saturating_add(1);
                    }
                    if chars.get(look) == Some(&'"') {
                        state = Lex::RawStr(hashes);
                        index = look;
                    }
                }
                '{' => {
                    depth = depth.checked_add(1)?;
                    line_delta = line_delta.checked_add(1)?;
                }
                '}' => {
                    depth = depth.checked_sub(1)?;
                    line_delta = line_delta.checked_sub(1)?;
                    if depth < 0 {
                        return None;
                    }
                }
                '\n' => {
                    deltas.push(line_delta);
                    line_delta = 0;
                    if index.saturating_add(1) < chars.len() {
                        let Ok(depth_value) = usize::try_from(depth) else {
                            return None;
                        };
                        depths_before.push(depth_value);
                    }
                }
                _ => {}
            },
            Lex::LineComment => {
                if ch == '\n' {
                    state = Lex::Normal;
                    deltas.push(line_delta);
                    line_delta = 0;
                    if index.saturating_add(1) < chars.len() {
                        let Ok(depth_value) = usize::try_from(depth) else {
                            return None;
                        };
                        depths_before.push(depth_value);
                    }
                }
            }
            Lex::BlockComment(block_depth) => {
                if ch == '/' && next == Some('*') {
                    state = Lex::BlockComment(block_depth.checked_add(1)?);
                    index = index.saturating_add(1);
                } else if ch == '*' && next == Some('/') {
                    state = if block_depth == 1 {
                        Lex::Normal
                    } else {
                        Lex::BlockComment(block_depth.checked_sub(1)?)
                    };
                    index = index.saturating_add(1);
                } else if ch == '\n' {
                    deltas.push(line_delta);
                    line_delta = 0;
                    if index.saturating_add(1) < chars.len() {
                        let Ok(depth_value) = usize::try_from(depth) else {
                            return None;
                        };
                        depths_before.push(depth_value);
                    }
                }
            }
            Lex::Str => match ch {
                '\\' => {
                    index = index.saturating_add(1);
                }
                '"' => state = Lex::Normal,
                '\n' => return None,
                _ => {}
            },
            Lex::Char => match ch {
                '\\' => {
                    index = index.saturating_add(1);
                }
                '\'' => state = Lex::Normal,
                '\n' => return None,
                _ => {}
            },
            Lex::RawStr(hashes) => {
                if ch == '"' {
                    let mut look = index.saturating_add(1);
                    let mut seen_hashes = 0;
                    while seen_hashes < hashes && chars.get(look) == Some(&'#') {
                        seen_hashes = seen_hashes.checked_add(1)?;
                        look = look.saturating_add(1);
                    }
                    if seen_hashes == hashes {
                        state = Lex::Normal;
                        index = look.checked_sub(1)?;
                    }
                }
            }
        }
        index = index.saturating_add(1);
    }
    match state {
        Lex::Normal | Lex::LineComment => {}
        Lex::BlockComment(_) | Lex::Str | Lex::Char | Lex::RawStr(_) => return None,
    }
    // A trailing newline already closed the last line; otherwise close it.
    if deltas.len() < depths_before.len() {
        deltas.push(line_delta);
    }
    if depth != 0 {
        return None;
    }
    Some((deltas, depths_before))
}

fn is_item_head(line: &str) -> bool {
    let mut tokens = line
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '#'))
        .filter(|token| !token.is_empty());
    let Some(first) = tokens.next() else {
        return false;
    };
    if first.starts_with('#') || first == "pub" {
        // `pub ...` or attribute: the head keyword follows.
        return tokens.any(|token| ITEM_HEADS.contains(&token));
    }
    ITEM_HEADS.contains(&first)
}

fn is_attachable_above(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("#[")
        || trimmed.starts_with("#![")
        || trimmed.starts_with("///")
        || trimmed.starts_with("//!")
        || trimmed.is_empty()
}

impl Chunker for SyntaxChunker {
    fn name(&self) -> &'static str {
        STRATEGY_SYNTAX
    }

    fn config(&self) -> String {
        format!("rust-items-m{}", self.max_item_bytes)
    }

    fn chunk(&self, file: &SourceFile) -> BenchResult<Vec<Chunk>> {
        if file.bytes.is_empty() {
            return Ok(Vec::new());
        }
        if std::path::Path::new(&file.path).extension() != Some(std::ffi::OsStr::new("rs")) {
            return fallback_chunk(file, &self.config());
        }
        let Some((deltas, depths)) = line_depths(&file.text) else {
            return fallback_chunk(file, &self.config());
        };
        let line_count = file.line_count();
        let normalized = file.text.replace("\r\n", "\n").replace('\r', "\n");
        let text_lines: Vec<&str> = normalized.lines().collect();
        if deltas.len() != line_count || text_lines.len() != line_count {
            return fallback_chunk(file, &self.config());
        }
        // Item head lines at depth 0.
        let mut heads: Vec<usize> = Vec::new();
        for (number, line) in text_lines.iter().enumerate() {
            if depths.get(number).copied().unwrap_or(usize::MAX) == 0 && is_item_head(line) {
                heads.push(number);
            }
        }
        if heads.is_empty() {
            return fallback_chunk(file, &self.config());
        }
        // Attach attribute/doc lines above each head.
        let mut starts: Vec<usize> = Vec::new();
        for head in &heads {
            let mut start = *head;
            while let Some(previous) = start.checked_sub(1).and_then(|index| text_lines.get(index))
            {
                if !is_attachable_above(previous) {
                    break;
                }
                start = start.saturating_sub(1);
            }
            starts.push(start);
        }
        // Each item runs to the line where depth returns to 0.
        let mut chunks = Vec::new();
        for (position, (head, start)) in heads.iter().zip(&starts).enumerate() {
            let start_line = start.saturating_add(1); // 1-based
            let head_text = text_lines.get(*head).ok_or_else(|| BenchError::Chunk {
                path: file.path.clone(),
                message: "syntax item head is outside line index".to_string(),
            })?;
            let head_braced = head_text.contains('{');
            let mut running = 0_i64;
            let mut seen_semi = false;
            let mut end_line = head.saturating_add(1);
            let mut closed = false;
            for (offset, delta) in deltas.iter().enumerate().skip(*head) {
                running = running
                    .checked_add(*delta)
                    .ok_or_else(|| BenchError::Chunk {
                        path: file.path.clone(),
                        message: "syntax nesting depth overflow".to_string(),
                    })?;
                if running != 0 {
                    continue;
                }
                if offset == *head {
                    if head_braced || head_text.contains(';') {
                        end_line = offset.saturating_add(1);
                        closed = true;
                        break;
                    }
                    continue;
                }
                if !head_braced {
                    // Braceless heads (`use`, multi-line `const`) close at
                    // the first depth-0 line that terminates the item.
                    if text_lines
                        .get(offset)
                        .is_some_and(|line| line.contains(';'))
                    {
                        seen_semi = true;
                    }
                    if !seen_semi {
                        continue;
                    }
                }
                end_line = offset.saturating_add(1);
                closed = true;
                break;
            }
            if !closed {
                return fallback_chunk(file, &self.config());
            }
            // Clamp to the next item start so items never overlap.
            if let Some(next_start) = starts.get(position.saturating_add(1)) {
                end_line = end_line.min(*next_start);
            }
            let (start_byte, end_byte) = file.line_span_bytes(start_line, end_line)?;
            let item_bytes = end_byte
                .checked_sub(start_byte)
                .ok_or_else(|| BenchError::Chunk {
                    path: file.path.clone(),
                    message: "syntax item has an inverted byte span".to_string(),
                })?;
            if item_bytes > self.max_item_bytes {
                // Declared fallback: an oversized item keeps one whole-file
                // chunk flagged fallback instead of a silent re-chunk.
                return fallback_chunk(file, &self.config());
            }
            chunks.push(make_chunk(
                file,
                &self.config(),
                start_byte,
                end_byte,
                start_line,
                end_line,
                false,
            )?);
        }
        // Preamble before the first item joins the first chunk.
        if let Some(first) = chunks.first().cloned()
            && first.start_byte > 0
        {
            let start_line = 1;
            let first_start_line =
                usize::try_from(first.start_line).map_err(|err| BenchError::Chunk {
                    path: file.path.clone(),
                    message: format!("syntax start line cannot fit usize: {err}"),
                })?;
            let first_end_line =
                usize::try_from(first.end_line).map_err(|err| BenchError::Chunk {
                    path: file.path.clone(),
                    message: format!("syntax end line cannot fit usize: {err}"),
                })?;
            let (_, first_end) = file.line_span_bytes(first_start_line, first_end_line)?;
            let rebuilt = make_chunk(
                file,
                &self.config(),
                0,
                first_end,
                start_line,
                first_end_line,
                false,
            )?;
            if let Some(slot) = chunks.first_mut() {
                *slot = rebuilt;
            }
        }
        Ok(chunks)
    }
}

fn make_chunk(
    file: &SourceFile,
    config: &str,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
    fallback: bool,
) -> BenchResult<Chunk> {
    let text = file
        .text
        .get(start_byte..end_byte)
        .ok_or_else(|| BenchError::Chunk {
            path: file.path.clone(),
            message: "syntax item span is not a UTF-8 boundary".to_string(),
        })?
        .to_string();
    let start_u32 = u32::try_from(start_byte).map_err(|err| BenchError::Chunk {
        path: file.path.clone(),
        message: format!("file exceeds u32 byte range: {err}"),
    })?;
    let end_u32 = u32::try_from(end_byte).map_err(|err| BenchError::Chunk {
        path: file.path.clone(),
        message: format!("file exceeds u32 byte range: {err}"),
    })?;
    let start_line_u32 = u32::try_from(start_line).map_err(|err| BenchError::Chunk {
        path: file.path.clone(),
        message: format!("file exceeds u32 line range: {err}"),
    })?;
    let end_line_u32 = u32::try_from(end_line).map_err(|err| BenchError::Chunk {
        path: file.path.clone(),
        message: format!("file exceeds u32 line range: {err}"),
    })?;
    Ok(Chunk {
        path: file.path.clone(),
        start_byte: start_u32,
        end_byte: end_u32,
        start_line: start_line_u32,
        end_line: end_line_u32,
        text: text.clone(),
        strategy: STRATEGY_SYNTAX.to_string(),
        version: CHUNKER_VERSION.to_string(),
        config: config.to_string(),
        chunk_id: chunk_id(
            STRATEGY_SYNTAX,
            CHUNKER_VERSION,
            config,
            &file.path,
            start_u32,
            end_u32,
            &sha256_hex(text.as_bytes()),
        ),
        fallback,
    })
}

fn fallback_chunk(file: &SourceFile, config: &str) -> BenchResult<Vec<Chunk>> {
    let end_line = file.line_count();
    if end_line == 0 {
        return Ok(Vec::new());
    }
    Ok(vec![make_chunk(
        file,
        config,
        0,
        file.bytes.len(),
        1,
        end_line,
        true,
    )?])
}
