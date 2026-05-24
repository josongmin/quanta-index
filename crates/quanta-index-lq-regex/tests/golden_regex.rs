//! Golden integration tests for LEX-04.
//!
//! Each row pins a (pattern, fixture-doc-set) pair and the expected
//! verified [`DocId`] set. The runner is fully deterministic: same
//! input → byte-identical output.

use quanta_index_lq_regex::{DocId, DocResolver, RegexErrorCode, RegexExecutor};
use std::collections::BTreeMap;

struct Map(BTreeMap<DocId, Vec<u8>>);

impl DocResolver for Map {
    fn resolve(&self, doc_id: DocId) -> Option<&[u8]> {
        self.0.get(&doc_id).map(Vec::as_slice)
    }
}

fn corpus() -> (Map, Vec<DocId>) {
    let docs: &[(DocId, &[u8])] = &[
        (
            DocId(1),
            b"fn handle_request(req: &Request) -> Response { todo() }",
        ),
        (
            DocId(2),
            b"fn handle_response(resp: &Response) -> () { ok() }",
        ),
        (DocId(3), b"pub fn other(arg: i32) -> bool { true }"),
        (DocId(4), b"struct Handler { state: u64 }"),
        (
            DocId(5),
            b"impl Handler { fn new() -> Self { Self::default() } }",
        ),
        (
            DocId(6),
            b"// fn fake_in_comment shouldn't match anchored search",
        ),
        (DocId(7), b"fn foo() {}"),
        (DocId(8), b"// fn foo - not a function definition"),
    ];
    let mut m: BTreeMap<DocId, Vec<u8>> = BTreeMap::new();
    let mut ids: Vec<DocId> = Vec::with_capacity(docs.len());
    for (d, bytes) in docs {
        let prior = m.insert(*d, bytes.to_vec());
        assert!(prior.is_none());
        ids.push(*d);
    }
    (Map(m), ids)
}

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

#[test]
fn golden_handler_function_regex() {
    let exec = match RegexExecutor::compile(r"fn\s+handle_\w+") {
        Ok(x) => x,
        Err(e) => fatal(&format!("{e}")),
    };
    let (m, ids) = corpus();
    match exec.execute_with_budget(&ids, &m, 0) {
        Ok(v) => assert_eq!(v, vec![DocId(1), DocId(2)]),
        Err(e) => fatal(&format!("{e}")),
    }
}

#[test]
fn golden_anchored_fn_foo() {
    let exec = match RegexExecutor::compile(r"^fn foo") {
        Ok(x) => x,
        Err(e) => fatal(&format!("{e}")),
    };
    let (m, ids) = corpus();
    match exec.execute_with_budget(&ids, &m, 0) {
        // Doc 7 starts with `fn foo`; doc 8 has `fn foo` but in a
        // comment that does not begin with `fn` at line start (multi-
        // line `^` is default-off).
        Ok(v) => assert_eq!(v, vec![DocId(7)]),
        Err(e) => fatal(&format!("{e}")),
    }
}

#[test]
fn golden_struct_or_impl_alternation() {
    let exec = match RegexExecutor::compile(r"^(struct|impl) Handler") {
        Ok(x) => x,
        Err(e) => fatal(&format!("{e}")),
    };
    let (m, ids) = corpus();
    match exec.execute_with_budget(&ids, &m, 0) {
        Ok(v) => assert_eq!(v, vec![DocId(4), DocId(5)]),
        Err(e) => fatal(&format!("{e}")),
    }
}

#[test]
fn golden_required_literals_for_handler() {
    let exec = match RegexExecutor::compile(r"fn\s+handle_\w+") {
        Ok(x) => x,
        Err(e) => fatal(&format!("{e}")),
    };
    let lits = match exec.required_literals() {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    // At least one extracted literal must begin with `fn`.
    let any = lits.iter().any(|l| l.starts_with(b"fn"));
    assert!(any, "literals: {lits:?}");
}

#[test]
fn golden_pure_wildcard_typed_unusable() {
    let exec = match RegexExecutor::compile(".*") {
        Ok(x) => x,
        Err(e) => fatal(&format!("{e}")),
    };
    match exec.required_literals() {
        Ok(v) => fatal(&format!("expected REGEX_PREFILTER_UNUSABLE, got {v:?}")),
        Err(e) => assert_eq!(e.code, RegexErrorCode::RegexPrefilterUnusable),
    }
}
