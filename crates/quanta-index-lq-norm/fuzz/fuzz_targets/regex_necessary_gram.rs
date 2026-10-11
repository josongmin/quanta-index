#![no_main]
#![forbid(unsafe_code)]

//! Exact regex truth must imply membership in its raw-byte gram candidates.
//! The posting source below scans the document bytes directly; it shares no
//! extraction or posting-builder logic with the implementation under test.

use libfuzzer_sys::fuzz_target;
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_trigram::{
    DocId, NecessaryGramCandidates, Trigram, TrigramError, TrigramPostingSource,
    evaluate_necessary_gram_expr,
};

const MAX_INPUT_BYTES: usize = 386;
const MAX_PATTERN_BYTES: usize = 128;
const MAX_DOCUMENT_BYTES: usize = 256;
const DOCUMENT_ID: DocId = DocId(1);

struct OneDocument<'a>(&'a [u8]);

impl TrigramPostingSource for OneDocument<'_> {
    fn intersect_trigrams(&self, grams: &[Trigram]) -> Result<Vec<DocId>, TrigramError> {
        if !grams.is_empty()
            && grams
                .iter()
                .all(|gram| self.0.windows(3).any(|window| window == gram))
        {
            Ok(vec![DOCUMENT_ID])
        } else {
            Ok(Vec::new())
        }
    }
}

fn bounded_text(bytes: &[u8], max_bytes: usize) -> String {
    // Lossy UTF-8 keeps arbitrary fuzzer bytes in play. The character cap
    // also bounds expansion from replacement characters to max_bytes.
    String::from_utf8_lossy(&bytes[..bytes.len().min(max_bytes)])
        .chars()
        .take(max_bytes / 4)
        .collect()
}

fn case(data: &[u8]) -> (String, String) {
    let data = &data[..data.len().min(MAX_INPUT_BYTES)];
    let mode = data.first().copied().unwrap_or(0) % 8;
    let selector = data.get(1).copied().unwrap_or(0);
    let payload = data.get(2..).unwrap_or_default();
    let seed = bounded_text(payload, 128);
    let word = ["foo", "bar", "abc", "k9", "xyz"][usize::from(selector) % 5];
    let suffix = ["end", "bar", "z9"][usize::from(selector >> 3) % 3];
    let short_branch = selector & 0x80 != 0;
    match mode {
        0 => {
            let cut = usize::from(selector) % payload.len().saturating_add(1);
            (
                bounded_text(&payload[..cut], MAX_PATTERN_BYTES),
                bounded_text(&payload[cut..], MAX_DOCUMENT_BYTES),
            )
        }
        1 => (
            format!("a|{word}"),
            if short_branch {
                format!("a{seed}")
            } else {
                format!("{seed}{word}")
            },
        ),
        2 => (format!("(?:|{word})"), seed),
        3 => (
            format!("(?:{word})?{suffix}"),
            if short_branch {
                format!("{seed}{suffix}")
            } else {
                format!("{word}{seed}{suffix}")
            },
        ),
        4 => (
            format!("(?i){word}"),
            format!(
                "{seed}{}",
                if short_branch {
                    word.to_uppercase()
                } else {
                    word.into()
                }
            ),
        ),
        5 => {
            let (pattern, lower, upper) = [
                ("é", "é", "É"),
                ("日本", "日本", "日本"),
                ("ſoo", "ſoo", "FOO"),
                ("Kelvin", "Kelvin", "KELVIN"),
            ][usize::from(selector) % 4];
            (
                format!("(?i){pattern}"),
                format!("{seed}{}", if short_branch { upper } else { lower }),
            )
        }
        6 => (format!("{word}.*{suffix}"), format!("{word}{seed}{suffix}")),
        _ => (
            format!("(?:{word}|x)"),
            if short_branch {
                format!("{seed}x")
            } else {
                format!("{seed}{word}")
            },
        ),
    }
}

fuzz_target!(|data: &[u8]| {
    let (pattern, document) = case(data);
    assert!(pattern.len() <= MAX_PATTERN_BYTES);
    assert!(document.len() <= MAX_DOCUMENT_BYTES);
    // Invalid/unsupported patterns have no exact-match implication to test.
    // A refusal is skipped; it is never treated as a successful candidate.
    let Ok(executor) = RegexExecutor::compile(&pattern) else {
        return;
    };
    if !executor.verify(document.as_bytes()) {
        return;
    }
    let expr = executor.necessary_gram_expr();
    let candidates = evaluate_necessary_gram_expr(&OneDocument(document.as_bytes()), &expr)
        .unwrap_or_else(|error| panic!("bounded gram evaluation refused: {error}; {expr:?}"));
    assert!(
        matches!(candidates, NecessaryGramCandidates::AllDocs)
            || matches!(candidates, NecessaryGramCandidates::Docs(ref ids) if ids.contains(&DOCUMENT_ID)),
        "regex match excluded its document: pattern={pattern:?}, document={document:?}, expression={expr:?}"
    );
});
