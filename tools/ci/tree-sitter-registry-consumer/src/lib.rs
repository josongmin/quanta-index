//! Isolated registry grammar checks; vendor C symbols must not be linked here.

#[path = "../../../../benchmarks/retrieval/src/symbols.rs"]
pub mod symbols;

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

mod corpus {
    // Mirrors the benchmark corpus line model used by the shared producer.
    pub(crate) fn split_line_starts(text: &str) -> (Vec<usize>, bool) {
        if text.is_empty() {
            return (Vec::new(), false);
        }
        let mut starts = vec![0_usize];
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes.get(index).copied() {
                Some(b'\r') => {
                    index += usize::from(bytes.get(index + 1) == Some(&b'\n')) + 1;
                    if index < bytes.len() {
                        starts.push(index);
                    }
                }
                Some(b'\n') => {
                    index += 1;
                    if index < bytes.len() {
                        starts.push(index);
                    }
                }
                _ => index += 1,
            }
        }
        (starts, false)
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::{Language, Parser, Query};

    const PRODUCER: &str = include_str!("../../../../benchmarks/retrieval/src/symbols.rs");

    fn producer_query(name: &str) -> &str {
        let marker = format!("const {name}: &str = r\"");
        let (_, after) = PRODUCER.split_once(&marker).expect("producer query exists");
        after.split_once("\";").expect("producer query closes").0
    }

    fn grammar_accepts(language: Language, query_name: &str, input: &str) -> bool {
        Query::new(&language, producer_query(query_name)).expect("producer query compiles");
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("grammar ABI is supported");
        !parser
            .parse(input, None)
            .expect("parser returned a tree")
            .root_node()
            .has_error()
    }

    #[test]
    fn go_projection_uses_registry_grammar_and_current_query() {
        let grammar: Language = tree_sitter_go::LANGUAGE.into();
        assert!(!grammar_accepts(
            grammar.clone(),
            "GO_QUERY",
            "package p\nfunc f(){_=new(1)}"
        ));
        assert!(grammar_accepts(
            grammar,
            "GO_QUERY",
            "package p\nfunc f(){_=foo(1)}"
        ));
    }

    #[test]
    fn javascript_projection_uses_registry_grammar_and_current_query() {
        let grammar: Language = tree_sitter_javascript::LANGUAGE.into();
        assert!(!grammar_accepts(
            grammar.clone(),
            "JAVASCRIPT_QUERY",
            "function local() {} export { local as const };"
        ));
        assert!(grammar_accepts(
            grammar,
            "JAVASCRIPT_QUERY",
            "function local() {} export { local as aaaaa };"
        ));
    }

    #[test]
    fn typescript_projection_uses_registry_grammar_and_current_query() {
        let grammar: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        assert!(!grammar_accepts(
            grammar.clone(),
            "TYPESCRIPT_QUERY",
            "export type * as N from './m'"
        ));
        assert!(grammar_accepts(
            grammar,
            "TYPESCRIPT_QUERY",
            "export      * as N from './m'"
        ));
    }

    #[test]
    fn tsx_projection_uses_registry_grammar_and_current_query() {
        let grammar: Language = tree_sitter_typescript::LANGUAGE_TSX.into();
        assert!(!grammar_accepts(
            grammar.clone(),
            "TYPESCRIPT_QUERY",
            "export type * as N from './m'; function f(){return <div/>}"
        ));
        assert!(grammar_accepts(
            grammar,
            "TYPESCRIPT_QUERY",
            "export      * as N from './m'; function f(){return <div/>}"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn official_symbol_records_match_fixed_source_goldens() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        use crate::sha256_hex;
        use crate::symbols::{
            SyntaxHelperArtifacts, extract_go_candidate_symbols_from_helper,
            extract_syntax_candidate_symbols_from_helper,
        };

        let directory = tempfile::tempdir().expect("fixture directory");
        let helper = directory.path().join("helper");
        let compiler = directory.path().join("typescript.js");
        fs::write(&compiler, "module.exports = {};").expect("compiler fixture");
        let compiler_digest = sha256_hex(b"module.exports = {};");
        let script_digest = sha256_hex(include_bytes!(
            "../../../../tools/benchmark/retrieval/ts_projection.cjs"
        ));
        for (producer, path, source, old, new, expected) in [
            (
                "go",
                "candidate.go",
                "package p\ntype Rect struct{W float64}\nfunc Run(){_=new(1)}\n",
                "new",
                "foo",
                &[
                    (
                        "sym-v1:5744ca145186f7e8aea95e84a63714958550cbe4ad2758fe4a3e8503247737ad",
                        "struct",
                        "Rect",
                        15_u64,
                        37_u64,
                        2_u64,
                    ),
                    (
                        "sym-v1:9e4dfdd0ddce9031bca33ebf09b934ed43970678e7a9558717b4886969f48bf5",
                        "function",
                        "Run",
                        38,
                        58,
                        3,
                    ),
                ][..],
            ),
            (
                "syntax",
                "alias.js",
                "function local() {} export { local as const };",
                "const",
                "aaaaa",
                &[(
                    "sym-v1:2153ffb6fa11a550bb90aadc64aa42fc0b6c7c77237a370eaf898ac1c4e75028",
                    "function",
                    "local",
                    0,
                    19,
                    1,
                )][..],
            ),
            (
                "syntax",
                "star.ts",
                "export type * as HttpProxy from './basic'\nfunction sentinel() {}",
                "type ",
                "     ",
                &[(
                    "sym-v1:e5cad1dfee7f4b8569052ba0443be007aa1a4a8b62c798da2037cac461648af3",
                    "function",
                    "sentinel",
                    42,
                    64,
                    2,
                )][..],
            ),
            (
                "syntax",
                "star.tsx",
                "export type * as HttpProxy from './basic'\nfunction sentinel() { return <div/> }",
                "type ",
                "     ",
                &[(
                    "sym-v1:8d134297bed70d64c2835d7730cbc4b1b2f8cdde1b9f3a73841c24b372e796bd",
                    "function",
                    "sentinel",
                    42,
                    79,
                    2,
                )][..],
            ),
        ] {
            let start = source.find(old).expect("projection target");
            let edit = serde_json::json!({
                "start": start,
                "end": start + old.len(),
                "replacement": new,
            });
            let response = serde_json::json!({
                "schema": if producer == "go" { "quanta-go-grammar-projection-v1" } else { "quanta-ts-grammar-projection-v1" },
                "status": "ok",
                "go_version": "go1.25.4",
                "node_version": "v-test",
                "typescript_version": "5.9.3",
                "source_sha256": sha256_hex(source.as_bytes()),
                "edits": [edit],
            });
            let mut response = response.as_object().expect("object").clone();
            if producer == "go" {
                response.remove("node_version");
                response.remove("typescript_version");
            } else {
                response.remove("go_version");
            }
            let script = format!(
                "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
                serde_json::Value::Object(response)
            );
            fs::write(&helper, &script).expect("helper fixture");
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o700))
                .expect("helper executable");
            let digest = sha256_hex(script.as_bytes());
            let records = if producer == "go" {
                extract_go_candidate_symbols_from_helper(
                    path,
                    source,
                    &helper,
                    &digest,
                    "go1.25.4",
                    tree_sitter_go::LANGUAGE.into(),
                    Instant::now() + Duration::from_secs(5),
                    || false,
                )
                .expect("official Go records")
                .records
            } else {
                let artifacts = SyntaxHelperArtifacts {
                    node: &helper,
                    node_sha256: &digest,
                    node_version: "v-test",
                    typescript_js: &compiler,
                    typescript_sha256: &compiler_digest,
                    script_sha256: &script_digest,
                    node_libraries: &[],
                };
                let grammar = match path {
                    "alias.js" => tree_sitter_javascript::LANGUAGE.into(),
                    "star.ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                    "star.tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
                    _ => unreachable!(),
                };
                extract_syntax_candidate_symbols_from_helper(
                    path,
                    source,
                    &artifacts,
                    grammar,
                    Instant::now() + Duration::from_secs(5),
                    || false,
                )
                .expect("official syntax records")
                .records
            };
            let values = serde_json::to_value(records).expect("records JSON");
            let values = values.as_array().expect("record array");
            assert_eq!(values.len(), expected.len(), "{path}");
            for (value, (id, kind, name, byte_start, byte_end, line)) in
                values.iter().zip(expected.iter())
            {
                assert_eq!(value["symbol_id"].as_str(), Some(*id), "{path}");
                assert_eq!(value["repo_relative_path"].as_str(), Some(path));
                assert_eq!(value["symbol_kind"].as_str(), Some(*kind), "{path}");
                assert_eq!(value["local_name"].as_str(), Some(*name), "{path}");
                assert_eq!(
                    value["definition_span"]["byte_start"].as_u64(),
                    Some(*byte_start)
                );
                assert_eq!(
                    value["definition_span"]["byte_end"].as_u64(),
                    Some(*byte_end)
                );
                assert_eq!(value["definition_span"]["line_start"].as_u64(), Some(*line));
                assert_eq!(value["relationship"].as_str(), Some("Def"));
            }
        }
    }
}
