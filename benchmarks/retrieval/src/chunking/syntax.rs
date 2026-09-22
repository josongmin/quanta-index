//! Syntax-aware chunker for the pilot language (Rust).
//!
//! A brace-aware splitter with a small lexer (comments, strings, chars,
//! raw strings) cuts top-level items into one chunk each. Oversized items
//! split by nested fixed windows; unbalanced input, empty item sets, and
//! non-Rust files fall back to an explicit whole-file chunk flagged
//! `fallback: true`, counted in coverage, never silent.

use crate::chunking::{CHUNKER_VERSION, STRATEGY_SYNTAX, Chunk, Chunker, chunk_id};
use crate::corpus::SourceFile;
use crate::{BenchError, BenchResult, sha256_hex};

/// Default per-item byte cap before nested fixed-window splitting.
pub const DEFAULT_MAX_ITEM_BYTES: usize = 32 * 1024;

const ITEM_HEADS: [&str; 15] = [
    "fn", "struct", "enum", "impl", "trait", "mod", "const", "static", "type", "use",
    "extern", "macro", "union", "macro_rules", "pub",
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

/// Per-line brace deltas plus the depth before each line. Strings and
/// comments never contribute braces. Returns `None` on unterminated input.
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
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        match state {
            Lex::Normal => match ch {
                '/' if next == Some('/') => state = Lex::LineComment,
                '/' if next == Some('*') => {
                    state = Lex::BlockComment(1);
                    index += 1;
                }
                '"' => state = Lex::Str,
                '\'' => state = Lex::Char,
                'r' if next == Some('"') || next == Some('#') => {
                    let mut hashes = 0;
                    let mut look = index + 1;
                    while chars.get(look) == Some(&'#') {
                        hashes += 1;
                        look += 1;
                    }
                    if chars.get(look) == Some(&'"') {
                        state = Lex::RawStr(hashes);
                        index = look;
                    } else {
                        }
                }
                '{' => {
                    depth += 1;
                    line_delta += 1;
                }
                '}' => {
                    depth -= 1;
                    line_delta -= 1;
                    if depth < 0 {
                        return None;
                    }
                }
                '\n' => {
                    deltas.push(line_delta);
                    line_delta = 0;
                    if index + 1 < chars.len() {
                        depths_before.push(usize::try_from(depth).unwrap_or(0));
                    }
                }
                _ => {}
            },
            Lex::LineComment => {
                if ch == '\n' {
                    state = Lex::Normal;
                    deltas.push(line_delta);
                    line_delta = 0;
                    if index + 1 < chars.len() {
                        depths_before.push(usize::try_from(depth).unwrap_or(0));
                    }
                }
            }
            Lex::BlockComment(nest) => {
                if ch == '/' && next == Some('*') {
                    state = Lex::BlockComment(nest + 1);
                    index += 1;
                } else if ch == '*' && next == Some('/') {
                    state = if nest == 1 {
                        Lex::Normal
                    } else {
                        Lex::BlockComment(nest - 1)
                    };
                    index += 1;
                } else if ch == '\n' {
                    deltas.push(line_delta);
                    line_delta = 0;
                    if index + 1 < chars.len() {
                        depths_before.push(usize::try_from(depth).unwrap_or(0));
                    }
                }
            }
            Lex::Str => match ch {
                '\\' => {
                    index += 1;
                }
                '"' => state = Lex::Normal,
                '\n' => return None,
                _ => {}
            },
            Lex::Char => match ch {
                '\\' => {
                    index += 1;
                }
                '\'' => state = Lex::Normal,
                '\n' => return None,
                _ => {}
            },
            Lex::RawStr(hashes) => {
                if ch == '"' {
                    let mut look = index + 1;
                    let mut seen = 0;
                    while seen < hashes && chars.get(look) == Some(&'#') {
                        seen += 1;
                        look += 1;
                    }
                    if seen == hashes {
                        state = Lex::Normal;
                        index = look - 1;
                    }
                }
            }
        }
        index += 1;
    }
    match state {
        Lex::Normal | Lex::LineComment => {}
        _ => return None,
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
        if !file.path.ends_with(".rs") {
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
            while start > 0 && is_attachable_above(text_lines[start - 1]) {
                start -= 1;
            }
            starts.push(start);
        }
        // Each item runs to the line where depth returns to 0.
        let mut chunks = Vec::new();
        for (position, head) in heads.iter().enumerate() {
            let start_line = starts[position] + 1; // 1-based
            let head_braced = text_lines[*head].contains('{');
            let mut running = 0_i64;
            let mut seen_semi = false;
            let mut end_line = *head + 1;
            let mut closed = false;
            for (offset, delta) in deltas.iter().enumerate().skip(*head) {
                running += *delta;
                if running != 0 {
                    continue;
                }
                if offset == *head {
                    if head_braced || text_lines[offset].contains(';') {
                        end_line = offset + 1;
                        closed = true;
                        break;
                    }
                    continue;
                }
                if !head_braced {
                    // Braceless heads (`use`, multi-line `const`) close at
                    // the first depth-0 line that terminates the item.
                    if text_lines[offset].contains(';') {
                        seen_semi = true;
                    }
                    if !seen_semi {
                        continue;
                    }
                }
                end_line = offset + 1;
                closed = true;
                break;
            }
            if !closed {
                return fallback_chunk(file, &self.config());
            }
            // Clamp to the next item start so items never overlap.
            if position + 1 < starts.len() {
                end_line = end_line.min(starts[position + 1]);
            }
            let (start_byte, end_byte) = file.line_span_bytes(start_line, end_line)?;
            if end_byte - start_byte > self.max_item_bytes {
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
        if let Some(first) = chunks.first() {
            if first.start_byte > 0 {
                let start_line = 1;
                let (_, first_end) = file.line_span_bytes(first.start_line as usize, first.end_line as usize)?;
                let rebuilt = make_chunk(
                    file,
                    &self.config(),
                    0,
                    first_end,
                    start_line,
                    first.end_line as usize,
                    false,
                )?;
                chunks[0] = rebuilt;
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
    let text = file.text[start_byte..end_byte].to_string();
    let start_u32 = u32::try_from(start_byte).map_err(|_| BenchError::Chunk {
        path: file.path.clone(),
        message: "file exceeds u32 byte range".to_string(),
    })?;
    let end_u32 = u32::try_from(end_byte).map_err(|_| BenchError::Chunk {
        path: file.path.clone(),
        message: "file exceeds u32 byte range".to_string(),
    })?;
    let start_line_u32 = u32::try_from(start_line).map_err(|_| BenchError::Chunk {
        path: file.path.clone(),
        message: "file exceeds u32 line range".to_string(),
    })?;
    let end_line_u32 = u32::try_from(end_line).map_err(|_| BenchError::Chunk {
        path: file.path.clone(),
        message: "file exceeds u32 line range".to_string(),
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
