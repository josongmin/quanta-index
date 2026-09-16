//! Golden trigram intersect corpus — hand-pinned.
//!
//! 5-document corpus, 5 query cases. Expected `DocId` sets are pinned
//! and must not drift; any change to the trigram extraction, intersect
//! algorithm, or sort/dedup order will break the golden.

use std::collections::BTreeMap;

use quanta_index_lq_trigram::{
    DocId, DocResolver, TrigramIndex, TrigramIndexBuilder, query_raw_substring,
    regex_prefilter_any_of,
};

struct Map(BTreeMap<DocId, Vec<u8>>);

impl DocResolver for Map {
    fn resolve(&self, doc_id: DocId) -> Option<&[u8]> {
        self.0.get(&doc_id).map(Vec::as_slice)
    }
}

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

fn corpus() -> (TrigramIndex, Map) {
    // 5 documents covering: shared trigrams, code-like content, unicode,
    // and a doc with no overlap.
    let docs: &[(DocId, &[u8])] = &[
        (DocId(1), b"fn handle_request(req: Request) -> Response"),
        (DocId(2), b"fn handle_response(res: Response) -> Result"),
        (DocId(3), b"struct Handler { state: AppState }"),
        (DocId(4), b"impl Handler { fn new() -> Self {} }"),
        (DocId(5), b"// no overlapping tokens here"),
    ];
    let Ok(mut b) = TrigramIndexBuilder::new(7) else {
        fatal("builder");
    };
    let mut m: BTreeMap<DocId, Vec<u8>> = BTreeMap::new();
    for (d, bytes) in docs {
        b.add_doc(*d, bytes);
        let prior = m.insert(*d, bytes.to_vec());
        assert!(prior.is_none());
    }
    (b.finish(), Map(m))
}

#[test]
fn golden_substring_handle_underscore() {
    let (idx, m) = corpus();
    let v = match query_raw_substring(&idx, b"handle_", &m) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // Only docs 1 and 2 contain the substring "handle_".
    assert_eq!(v, vec![DocId(1), DocId(2)]);
}

#[test]
fn golden_substring_handler() {
    let (idx, m) = corpus();
    let v = match query_raw_substring(&idx, b"Handler", &m) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // Only docs 3 and 4 contain the substring "Handler".
    assert_eq!(v, vec![DocId(3), DocId(4)]);
}

#[test]
fn golden_substring_response_uppercase() {
    let (idx, m) = corpus();
    let v = match query_raw_substring(&idx, b"Response", &m) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // docs 1 and 2 contain "Response" (uppercase R).
    assert_eq!(v, vec![DocId(1), DocId(2)]);
}

#[test]
fn golden_substring_no_match() {
    let (idx, m) = corpus();
    let v = match query_raw_substring(&idx, b"ZzZzZz", &m) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert!(v.is_empty());
}

#[test]
fn golden_regex_prefilter_alternation_fn_or_handle() {
    let (idx, _m) = corpus();
    let lits: &[Vec<u8>] = &[b"fn ".to_vec(), b"handle_".to_vec()];
    let v = match regex_prefilter_any_of(&idx, lits) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // A literal extractor's output is an alternation: `/(fn |handle_)/` needs
    // one of them. Docs 1 and 2 hold both; doc 4 (`impl Handler { fn new() ...`)
    // holds only "fn " and must still be a candidate. The previous expectation
    // here was [1, 2] because the prefilter intersected across literals, which
    // is precisely the false negative this corpus now pins against. This is a
    // trigram-level candidate set, not a substring confirmation — verify stays
    // the caller's job on the regex path.
    assert_eq!(v, vec![DocId(1), DocId(2), DocId(4)]);
}

#[test]
fn golden_cbor_encoding_byte_pinned() {
    // The CBOR encoding must be deterministic. We don't pin the exact
    // bytes (encoding includes ciborium's choices) but we pin that two
    // builds produce the same length and the same bytes.
    let (idx1, _) = corpus();
    let (idx2, _) = corpus();
    let mut b1: Vec<u8> = Vec::new();
    let mut b2: Vec<u8> = Vec::new();
    if let Err(e) = idx1.serialize_cbor(&mut b1) {
        fatal(&format!("{e}"));
    }
    if let Err(e) = idx2.serialize_cbor(&mut b2) {
        fatal(&format!("{e}"));
    }
    assert_eq!(b1, b2);
    // The encoding is non-empty.
    assert!(!b1.is_empty());
}
