//! Parser compatibility regressions for CS-PROD-01.
//!
//! Handwritten sentinels and inventories define the expected source facts.
//! Calls and namespace re-exports must not invent symbol definitions.
//! These tests deliberately require a clean parse, never recovered-tree output.

#![expect(
    clippy::expect_used,
    reason = "fixture setup and source-span oracles fail explicitly on absence"
)]

use std::collections::BTreeMap;

use quanta_index_contract::lex::SymbolRelationship;
use quanta_index_contract::{
    RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision, SymbolCoverage,
    source_file_unit_set_sha256,
};
use quanta_index_retrieval_bench::corpus::SourceFile;
use quanta_index_retrieval_bench::sha256_hex;
use quanta_index_retrieval_bench::symbols::{
    SymbolCoveragePolicy, SymbolExtractError, SymbolPreflightOptions, extract_corpus_symbols,
    extract_symbols, preflight_corpus_symbols,
};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const GENERIC_IMPORT: &str = include_str!("fixtures/l5_parser/generic_import_trailing_comma.ts");
const TYPE_NAMESPACE_EXPORT: &str = include_str!("fixtures/l5_parser/type_namespace_export.ts");
const MALFORMED: &str = include_str!("fixtures/l5_parser/malformed.ts");
const SENTINEL: &str = "function sentinel() {}";

fn assert_only_sentinel(path: &str, source: &str, expected_line: u32) {
    let symbols = extract_symbols(path, source).expect("valid TypeScript must parse cleanly");
    assert_eq!(
        symbols.len(),
        1,
        "no call or re-export may invent a definition"
    );
    let symbol = symbols.first().expect("one sentinel");
    assert_eq!(symbol.local_name.as_ref(), "sentinel");
    assert_eq!(symbol.qualified_name.as_ref(), "sentinel");
    assert!(symbol.container_qualified_name.is_none());
    assert_eq!(symbol.symbol_kind.as_str(), "function");
    assert_eq!(symbol.language.as_str(), "typescript");
    assert_eq!(symbol.relationship, SymbolRelationship::Def);
    assert_eq!(symbol.repo_relative_path.as_str(), path);
    assert_eq!(symbol.definition_span.path.as_ref(), path);
    let start = source.find(SENTINEL).expect("handwritten sentinel");
    let end = start
        .checked_add(SENTINEL.len())
        .expect("small fixture span");
    assert_eq!(
        symbol.definition_span.byte_start,
        u32::try_from(start).expect("small fixture offset")
    );
    assert_eq!(
        symbol.definition_span.byte_end,
        u32::try_from(end).expect("small fixture offset")
    );
    assert_eq!(symbol.definition_span.line_start, expected_line);
    assert_eq!(symbol.definition_span.line_end, expected_line);
}

fn source_file(path: &str, text: &str) -> SourceFile {
    assert!(!text.contains('\r'), "LF-only fixture helper");
    SourceFile {
        path: path.to_string(),
        bytes: text.as_bytes().to_vec(),
        text: text.to_string(),
        line_starts: text
            .split_inclusive('\n')
            .scan(0usize, |offset, line| {
                let start = *offset;
                *offset = offset
                    .checked_add(line.len())
                    .expect("small fixture line offset");
                Some(start)
            })
            .collect(),
        sha256: sha256_hex(text.as_bytes()),
    }
}

#[test]
fn generic_typeof_import_call_accepts_a_trailing_comma() {
    for path in ["src/call.ts", "src/call.tsx"] {
        assert_only_sentinel(path, GENERIC_IMPORT, 2);
    }
}

#[test]
fn type_namespace_reexport_accepts_valid_syntax_without_inventing_a_definition() {
    for path in ["src/export.ts", "src/export.tsx"] {
        assert_only_sentinel(path, TYPE_NAMESPACE_EXPORT, 2);
    }
}

#[test]
fn ordinary_typescript_and_tsx_definitions_remain_source_bound() {
    for path in ["src/control.ts", "src/control.tsx"] {
        assert_only_sentinel(path, SENTINEL, 1);
    }
}

#[test]
fn private_methods_preserve_definition_identity_across_js_and_ts() {
    let source = "class Vault { #read() { function nested() {} } read() {} }";
    for path in [
        "vault.js",
        "vault.mjs",
        "vault.cjs",
        "vault.jsx",
        "vault.ts",
        "vault.tsx",
    ] {
        let files = BTreeMap::from([(path.to_string(), source_file(path, source))]);
        let preflight = preflight_corpus_symbols(&files, &SymbolPreflightOptions::default())
            .expect("valid private method source");
        preflight
            .admit(SymbolCoveragePolicy::RequireComplete)
            .expect("complete coverage");
        let records = preflight.symbols().get(path).expect("admitted file");
        let actual = records
            .iter()
            .map(|record| (record.qualified_name.as_ref(), record.symbol_kind.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            records.len(),
            4,
            "every named definition exactly once: {path}"
        );
        assert_eq!(
            actual,
            std::collections::BTreeSet::from([
                ("Vault", "class"),
                ("Vault.#read", "method"),
                ("Vault.#read.nested", "function"),
                ("Vault.read", "method"),
            ])
        );
        let method = records
            .iter()
            .find(|record| record.local_name.as_ref() == "#read")
            .expect("private method definition");
        let start = usize::try_from(method.definition_span.byte_start).expect("offset");
        let end = usize::try_from(method.definition_span.byte_end).expect("offset");
        assert_eq!(
            source.get(start..end),
            Some("#read() { function nested() {} }")
        );
        assert_eq!(
            preflight.report().files.first().expect("one file").coverage,
            SymbolCoverage::Complete { symbol_count: 4 }
        );
    }
}

// Handwritten inventories are independent of the producer query. Check exact
// multiplicity, ownership, kinds, source spans, and the published coverage count.
fn assert_definition_inventory(path: &str, source: &str, expected: &[(&str, &str, &str)]) {
    let files = BTreeMap::from([(path.to_string(), source_file(path, source))]);
    let preflight = preflight_corpus_symbols(&files, &SymbolPreflightOptions::default())
        .expect("valid fixture preflight");
    preflight
        .admit(SymbolCoveragePolicy::RequireComplete)
        .expect("complete inventory");
    let records = preflight.symbols().get(path).expect("admitted file");
    let actual = records
        .iter()
        .map(|record| {
            let start = usize::try_from(record.definition_span.byte_start).expect("small offset");
            let end = usize::try_from(record.definition_span.byte_end).expect("small offset");
            (
                record.qualified_name.as_ref(),
                record.symbol_kind.as_str(),
                source.get(start..end).expect("source-bound UTF-8 span"),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        actual,
        expected.iter().copied().collect(),
        "inventory: {path}"
    );
    assert_eq!(records.len(), expected.len(), "no duplicate definitions");
    for record in records {
        let qualified = record.qualified_name.as_ref();
        let owner = qualified.rsplit_once('.').map(|(owner, _)| owner);
        assert_eq!(record.container_qualified_name.as_deref(), owner);
        assert_eq!(
            record.local_name.as_ref(),
            qualified.rsplit('.').next().expect("name")
        );
    }
    assert_eq!(
        preflight.report().files.first().expect("one file").coverage,
        SymbolCoverage::Complete {
            symbol_count: u64::try_from(expected.len()).expect("count")
        }
    );
}

#[test]
fn go_aliases_and_local_types_keep_function_and_receiver_ownership() {
    let source = "package p\ntype Value struct{}\ntype Alias = Value\ntype Shape = struct{}\nfunc Outer() { type Local struct{}; type Copy = Local }\nfunc (v *Value) Read() { type Local struct{}; type Copy = Local }\n";
    assert_definition_inventory(
        "scope.go",
        source,
        &[
            ("Value", "struct", "Value struct{}"),
            ("Alias", "type_alias", "Alias = Value"),
            ("Shape", "type_alias", "Shape = struct{}"),
            (
                "Outer",
                "function",
                "func Outer() { type Local struct{}; type Copy = Local }",
            ),
            ("Outer.Local", "struct", "Local struct{}"),
            ("Outer.Copy", "type_alias", "Copy = Local"),
            (
                "Value.Read",
                "method",
                "func (v *Value) Read() { type Local struct{}; type Copy = Local }",
            ),
            ("Value.Read.Local", "struct", "Local struct{}"),
            ("Value.Read.Copy", "type_alias", "Copy = Local"),
        ],
    );
}

#[test]
fn go_named_interfaces_emit_owned_method_contracts() {
    let source = "package p\ntype Reader interface { Read() int }\ntype Alias = interface { Read() int }\nfunc Outer() { type Local interface { Read() int } }\n";
    assert_definition_inventory(
        "contracts.go",
        source,
        &[
            ("Reader", "interface", "Reader interface { Read() int }"),
            ("Reader.Read", "method", "Read() int"),
            ("Alias", "type_alias", "Alias = interface { Read() int }"),
            ("Alias.Read", "method", "Read() int"),
            (
                "Outer",
                "function",
                "func Outer() { type Local interface { Read() int } }",
            ),
            ("Outer.Local", "interface", "Local interface { Read() int }"),
            ("Outer.Local.Read", "method", "Read() int"),
        ],
    );
}

#[test]
fn named_expressions_keep_their_names_without_naming_anonymous_bindings() {
    let source = "const c = class Inner { run() {} };\nconst f = function Named() { function nested() {} };\nconst g = function* Generate() { function nested() {} };\nconst anonymousClass = class {};\nconst anonymousFunction = function() { function child() {} };\nconst anonymousGenerator = function*() {};\n";
    for path in ["expr.js", "expr.jsx", "expr.ts", "expr.tsx"] {
        assert_definition_inventory(
            path,
            source,
            &[
                ("Inner", "class", "class Inner { run() {} }"),
                ("Inner.run", "method", "run() {}"),
                (
                    "Named",
                    "function",
                    "function Named() { function nested() {} }",
                ),
                ("Named.nested", "function", "function nested() {}"),
                (
                    "Generate",
                    "function",
                    "function* Generate() { function nested() {} }",
                ),
                ("Generate.nested", "function", "function nested() {}"),
                ("child", "function", "function child() {}"),
            ],
        );
    }
}

#[test]
fn typescript_callable_declarations_keep_owners_and_overload_spans() {
    let source = "type Alias = { read(): string };\ninterface Reader { read(): string; }\nabstract class Base { abstract read(): string; }\ndeclare function load(): string;\nfunction pick(x: string): string;\nfunction pick(x: number): number;\nfunction pick(x: any): any { return x; }\nclass Impl { read(x: string): string; read(x: any): any { return x; } }\n";
    for path in ["decl.ts", "decl.tsx"] {
        assert_definition_inventory(
            path,
            source,
            &[
                ("Alias", "type_alias", "type Alias = { read(): string };"),
                ("Alias.read", "method", "read(): string"),
                (
                    "Reader",
                    "interface",
                    "interface Reader { read(): string; }",
                ),
                ("Reader.read", "method", "read(): string"),
                (
                    "Base",
                    "class",
                    "abstract class Base { abstract read(): string; }",
                ),
                ("Base.read", "method", "abstract read(): string"),
                ("load", "function", "function load(): string;"),
                ("pick", "function", "function pick(x: string): string;"),
                ("pick", "function", "function pick(x: number): number;"),
                (
                    "pick",
                    "function",
                    "function pick(x: any): any { return x; }",
                ),
                (
                    "Impl",
                    "class",
                    "class Impl { read(x: string): string; read(x: any): any { return x; } }",
                ),
                ("Impl.read", "method", "read(x: string): string"),
                ("Impl.read", "method", "read(x: any): any { return x; }"),
            ],
        );
    }
}

#[test]
fn typescript_anonymous_type_members_do_not_become_false_owner_methods() {
    let source = "type Direct = { run(): void };\ntype Nested = { child: { run(): void } };\nfunction use(arg: { run(): void }) {}\nclass Outer { field: { run(): void }; run(): void; }\n";
    for path in ["type-scope.ts", "type-scope.tsx"] {
        assert_definition_inventory(
            path,
            source,
            &[
                ("Direct", "type_alias", "type Direct = { run(): void };"),
                ("Direct.run", "method", "run(): void"),
                (
                    "Nested",
                    "type_alias",
                    "type Nested = { child: { run(): void } };",
                ),
                ("use", "function", "function use(arg: { run(): void }) {}"),
                (
                    "Outer",
                    "class",
                    "class Outer { field: { run(): void }; run(): void; }",
                ),
                ("Outer.run", "method", "run(): void"),
            ],
        );
        let only_function = "function use(arg: { run(): void }) {}";
        let files = BTreeMap::from([(path.to_string(), source_file(path, only_function))]);
        let options = SymbolPreflightOptions {
            max_symbols_per_file: 1,
            max_symbols_total: 1,
            ..SymbolPreflightOptions::default()
        };
        let preflight = preflight_corpus_symbols(&files, &options).expect("bounded preflight");
        preflight
            .admit(SymbolCoveragePolicy::RequireComplete)
            .expect("skipped anonymous signatures must not exhaust symbol budget");
        assert_eq!(
            preflight.report().files.first().expect("one file").coverage,
            SymbolCoverage::Complete { symbol_count: 1 }
        );
    }
}

#[test]
fn typescript_alias_composition_keeps_only_owned_method_signatures() {
    let source = "type Combined = { alpha(): void } & { beta(): void };\ntype Grouped = ({ gamma(): void });\ntype Generic<T> = Promise<{ delta(): void }>;\n";
    for path in ["composed.ts", "composed.tsx"] {
        assert_definition_inventory(
            path,
            source,
            &[
                (
                    "Combined",
                    "type_alias",
                    "type Combined = { alpha(): void } & { beta(): void };",
                ),
                ("Combined.alpha", "method", "alpha(): void"),
                ("Combined.beta", "method", "beta(): void"),
                (
                    "Grouped",
                    "type_alias",
                    "type Grouped = ({ gamma(): void });",
                ),
                ("Grouped.gamma", "method", "gamma(): void"),
                (
                    "Generic",
                    "type_alias",
                    "type Generic<T> = Promise<{ delta(): void }>;",
                ),
            ],
        );
    }
}

#[test]
fn typescript_named_modules_and_enums_are_definitions_too() {
    let source =
        "namespace Outer { export enum Color { Red, Blue } export function inside() {} }\n";
    for path in ["ns.ts", "ns.tsx"] {
        assert_definition_inventory(
            path,
            source,
            &[
                (
                    "Outer",
                    "module",
                    "namespace Outer { export enum Color { Red, Blue } export function inside() {} }",
                ),
                ("Outer.Color", "enum", "enum Color { Red, Blue }"),
                ("Outer.inside", "function", "function inside() {}"),
            ],
        );
    }
}

#[test]
fn dotted_namespaces_use_ast_segments_not_raw_spacing_or_comments() {
    let source = "namespace Outer . Inner { export function first() {} }\nnamespace A/* gap */. B . C { export class Item {} }\n";
    for path in ["dotted.ts", "dotted.tsx"] {
        assert_definition_inventory(
            path,
            source,
            &[
                (
                    "Outer.Inner",
                    "module",
                    "namespace Outer . Inner { export function first() {} }",
                ),
                ("Outer.Inner.first", "function", "function first() {}"),
                (
                    "A.B.C",
                    "module",
                    "namespace A/* gap */. B . C { export class Item {} }",
                ),
                ("A.B.C.Item", "class", "class Item {}"),
            ],
        );
    }
}

#[test]
fn empty_and_zero_definition_files_differ_from_parse_failure() {
    for path in ["src/empty.ts", "src/empty.tsx"] {
        for source in ["", "// No declarations.\n"] {
            assert!(
                extract_symbols(path, source)
                    .expect("valid empty inventory")
                    .is_empty()
            );
        }
        assert_eq!(
            extract_symbols(path, MALFORMED).expect_err("malformed source"),
            SymbolExtractError::ParseFailure {
                path: path.to_string(),
            }
        );
    }
}

#[test]
fn a_final_file_parse_failure_cannot_return_partial_corpus_success() {
    let files = BTreeMap::from([
        ("a.ts".to_string(), source_file("a.ts", SENTINEL)),
        ("m.ts".to_string(), source_file("m.ts", "")),
        ("z.ts".to_string(), source_file("z.ts", MALFORMED)),
    ]);
    let error = extract_corpus_symbols(&files)
        .err()
        .expect("strict publication must refuse the final invalid file");
    assert!(matches!(
        &error,
        quanta_index_retrieval_bench::BenchError::Chunk { path, .. } if path == "z.ts"
    ));
    assert!(
        error
            .to_string()
            .contains(&sha256_hex(MALFORMED.as_bytes()))
    );
}

#[test]
fn corpus_extraction_retains_admitted_empty_files() {
    let files = BTreeMap::from([
        ("a.ts".to_string(), source_file("a.ts", SENTINEL)),
        ("empty.ts".to_string(), source_file("empty.ts", "")),
    ]);
    let extraction = extract_corpus_symbols(&files).expect("complete admitted corpus");
    assert_eq!(extraction.symbols.len(), 2);
    assert!(
        extraction
            .symbols
            .get("empty.ts")
            .expect("admitted empty file")
            .is_empty()
    );
    assert_eq!(
        extraction
            .symbols
            .get("a.ts")
            .expect("named definition")
            .len(),
        1
    );
}

#[test]
fn preflight_reports_every_admitted_file_even_after_the_first_failure() {
    let files = BTreeMap::from([
        ("a.md".to_string(), source_file("a.md", "# unsupported")),
        ("b.ts".to_string(), source_file("b.ts", SENTINEL)),
        ("empty.ts".to_string(), source_file("empty.ts", "")),
        ("z.ts".to_string(), source_file("z.ts", MALFORMED)),
    ]);
    let preflight =
        preflight_corpus_symbols(&files, &SymbolPreflightOptions::default()).expect("census");
    assert_eq!(preflight.report().admitted_files, 4);
    assert_eq!(preflight.report().incomplete_files, 2);
    assert_eq!(
        preflight
            .report()
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["a.md", "b.ts", "empty.ts", "z.ts"]
    );
    let states = preflight
        .report()
        .files
        .iter()
        .map(|file| file.coverage)
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        [
            SymbolCoverage::Unsupported,
            SymbolCoverage::Complete { symbol_count: 1 },
            SymbolCoverage::Complete { symbol_count: 0 },
            SymbolCoverage::ParseFailed
        ]
    );
    assert!(
        preflight
            .admit(SymbolCoveragePolicy::RequireComplete)
            .is_err()
    );
    preflight
        .admit(SymbolCoveragePolicy::AllowIncomplete)
        .expect("explicit text policy");
    assert!(
        preflight
            .symbols()
            .get("z.ts")
            .expect("failed file accounted for")
            .is_empty()
    );
}

#[test]
fn bounded_diagnostics_retain_all_failed_file_counts() {
    let files = BTreeMap::from([
        ("a.ts".to_string(), source_file("a.ts", MALFORMED)),
        ("z.ts".to_string(), source_file("z.ts", MALFORMED)),
    ]);
    let options = SymbolPreflightOptions {
        max_diagnostics_total: 1,
        max_diagnostics_per_file: 1,
        ..SymbolPreflightOptions::default()
    };
    let preflight = preflight_corpus_symbols(&files, &options).expect("bounded census");
    assert_eq!(preflight.report().incomplete_files, 2);
    assert_eq!(
        preflight
            .report()
            .files
            .iter()
            .map(|file| file.diagnostics.len())
            .sum::<usize>(),
        1
    );
    let last = preflight
        .report()
        .files
        .last()
        .expect("last malformed file");
    assert!(last.diagnostics_total > 0);
    assert!(last.diagnostics_truncated);
    assert!(last.diagnostics_complete);
    assert_eq!(last.coverage, SymbolCoverage::ParseFailed);
}

#[test]
fn cancelled_and_timed_out_census_never_becomes_complete_zero() {
    let files = BTreeMap::from([
        ("a.ts".to_string(), source_file("a.ts", "")),
        ("z.ts".to_string(), source_file("z.ts", SENTINEL)),
    ]);
    let cancelled = AtomicBool::new(true);
    for (options, reason) in [
        (
            SymbolPreflightOptions {
                cancellation: Some(&cancelled),
                ..SymbolPreflightOptions::default()
            },
            "cancelled",
        ),
        (
            SymbolPreflightOptions {
                timeout_per_file: Duration::ZERO,
                ..SymbolPreflightOptions::default()
            },
            "timeout",
        ),
    ] {
        let preflight =
            preflight_corpus_symbols(&files, &options).expect("explicit aborted states");
        assert_eq!(preflight.report().admitted_files, 2);
        assert_eq!(preflight.report().incomplete_files, 2);
        for file in &preflight.report().files {
            assert_eq!(file.coverage, SymbolCoverage::ProducerFailed);
            assert_eq!(file.failure, Some(reason));
            assert!(!file.diagnostics_complete);
        }
        assert!(
            preflight
                .admit(SymbolCoveragePolicy::AllowIncomplete)
                .is_err()
        );
    }
}

#[test]
fn symbol_limit_refuses_partial_definition_output() {
    let files = BTreeMap::from([(
        "a.ts".to_string(),
        source_file("a.ts", "function first() {} function second() {}"),
    )]);
    let options = SymbolPreflightOptions {
        max_symbols_per_file: 1,
        ..SymbolPreflightOptions::default()
    };
    let preflight = preflight_corpus_symbols(&files, &options).expect("explicit resource refusal");
    let file = preflight.report().files.first().expect("one file");
    assert_eq!(file.coverage, SymbolCoverage::ProducerFailed);
    assert_eq!(file.failure, Some("resource_limit"));
    assert!(
        preflight
            .symbols()
            .get("a.ts")
            .expect("file retained")
            .is_empty()
    );
}

#[test]
fn fatal_producer_failure_is_not_masked_by_an_earlier_recoverable_gap() {
    let files = BTreeMap::from([
        ("a.md".to_string(), source_file("a.md", "unsupported")),
        ("z.ts".to_string(), source_file("z.ts", SENTINEL)),
    ]);
    let options = SymbolPreflightOptions {
        max_symbols_per_file: 0,
        ..SymbolPreflightOptions::default()
    };
    let preflight = preflight_corpus_symbols(&files, &options).expect("complete census");
    assert_eq!(preflight.report().incomplete_files, 2);
    for policy in [
        SymbolCoveragePolicy::RequireComplete,
        SymbolCoveragePolicy::AllowIncomplete,
    ] {
        let error = preflight
            .admit(policy)
            .expect_err("producer failure is fatal under either policy");
        assert!(matches!(
            error,
            quanta_index_retrieval_bench::BenchError::Protocol(_)
        ));
        assert!(error.to_string().contains("z.ts"));
    }
}

#[test]
fn producer_policy_records_and_commits_effective_extraction_limits() {
    let files = BTreeMap::from([("empty.ts".to_string(), source_file("empty.ts", ""))]);
    let options = SymbolPreflightOptions::default();
    let first = preflight_corpus_symbols(&files, &options).expect("default census");
    let changed = SymbolPreflightOptions {
        max_symbols_total: 7,
        timeout_total: Duration::from_secs(11),
        ..options
    };
    let second = preflight_corpus_symbols(&files, &changed).expect("changed limits census");
    assert_eq!(second.report().policy.max_symbols_total, 7);
    assert_eq!(second.report().policy.timeout_total_ns, "11000000000");
    assert_ne!(
        first.report().producer_policy_sha256,
        second.report().producer_policy_sha256
    );
    assert_eq!(
        first.report().grammar_identity,
        second.report().grammar_identity
    );
    assert_eq!(
        first.report().files.first().expect("first source").coverage,
        second
            .report()
            .files
            .first()
            .expect("second source")
            .coverage
    );
}

#[test]
fn file_and_total_symbol_budgets_refuse_publication_without_losing_the_census() {
    let files = BTreeMap::from([
        ("a.ts".to_string(), source_file("a.ts", SENTINEL)),
        ("m.ts".to_string(), source_file("m.ts", "")),
        ("z.ts".to_string(), source_file("z.ts", SENTINEL)),
    ]);
    let options = SymbolPreflightOptions {
        max_symbols_total: 1,
        ..SymbolPreflightOptions::default()
    };
    let preflight = preflight_corpus_symbols(&files, &options).expect("budget census");
    assert_eq!(
        preflight
            .report()
            .files
            .iter()
            .map(|row| row.coverage)
            .collect::<Vec<_>>(),
        [
            SymbolCoverage::Complete { symbol_count: 1 },
            SymbolCoverage::Complete { symbol_count: 0 },
            SymbolCoverage::ProducerFailed,
        ]
    );
    assert!(
        preflight
            .admit(SymbolCoveragePolicy::AllowIncomplete)
            .is_err()
    );
    assert!(
        preflight
            .symbols()
            .get("z.ts")
            .expect("last source")
            .is_empty()
    );
    let options = SymbolPreflightOptions {
        max_file_bytes: 0,
        ..SymbolPreflightOptions::default()
    };
    let preflight = preflight_corpus_symbols(&files, &options).expect("file budget census");
    assert_eq!(preflight.report().admitted_files, 3);
    assert_eq!(preflight.report().incomplete_files, 2);
    assert!(
        preflight
            .admit(SymbolCoveragePolicy::AllowIncomplete)
            .is_err()
    );
}

#[test]
fn preflight_rejects_forged_source_before_reporting_coverage() {
    for mode in 0..4 {
        let mut file = source_file("a.ts", SENTINEL);
        match mode {
            0 => file.path = "other.ts".to_string(),
            1 => file.sha256 = "0".repeat(64),
            2 => file.text = "function forged() {}".to_string(),
            _ => file.line_starts = vec![7],
        }
        let files = BTreeMap::from([("a.ts".to_string(), file)]);
        assert!(matches!(
            preflight_corpus_symbols(&files, &SymbolPreflightOptions::default()),
            Err(quanta_index_retrieval_bench::BenchError::Corpus { .. })
        ));
    }
    for (path, text) in [
        ("../a.ts", SENTINEL),
        ("a.ts", "// exotic\u{2028}function sentinel() {}"),
    ] {
        let file = source_file(path, text);
        let files = BTreeMap::from([(path.to_string(), file)]);
        assert!(matches!(
            preflight_corpus_symbols(&files, &SymbolPreflightOptions::default()),
            Err(quanta_index_retrieval_bench::BenchError::Corpus { .. })
        ));
    }
}

#[test]
fn empty_file_coverage_binds_shared_identity_and_canonical_empty_unit_set() {
    use sha2::{Digest, Sha256};
    let file = source_file("empty.ts", "");
    let files = BTreeMap::from([("empty.ts".to_string(), file.clone())]);
    let preflight = preflight_corpus_symbols(&files, &SymbolPreflightOptions::default())
        .expect("complete empty file");
    let source = SourceFileRevision {
        file: SourceFileKey {
            source_repo_id: RepoId::new("repo").expect("repo"),
            repo_relative_path: RepoRelativePath::new("empty.ts"),
        },
        revision_id: RevisionId::new("revision").expect("revision"),
        source_sha256: Sha256::digest(b"").into(),
    };
    let language = quanta_index_contract::lex::LanguageCode::new("typescript").expect("language");
    let coverage = preflight
        .coverage_for(source.clone(), language.clone(), &file, &[])
        .expect("shared coverage");
    assert_eq!(coverage.source, source);
    assert_eq!(
        coverage.symbols,
        SymbolCoverage::Complete { symbol_count: 0 }
    );
    assert!(coverage.text_admitted);
    assert_eq!(
        coverage.unit_set_sha256,
        source_file_unit_set_sha256(&[], &[]).expect("canonical empty set")
    );
    let mut changed = source;
    changed.source_sha256 = [7; 32];
    assert!(
        preflight
            .coverage_for(changed, language, &file, &[])
            .is_err()
    );
}

#[test]
fn preflight_preserves_existing_supported_language_extraction() {
    let cases = [
        ("one.rs", "fn sentinel() {}"),
        ("two.go", "package fixture\nfunc sentinel() {}\n"),
        ("three.py", "def sentinel():\n    pass\n"),
        ("four.js", SENTINEL),
        ("five.ts", SENTINEL),
        ("six.tsx", "function sentinel() { return <div/> }"),
    ];
    let files = cases
        .into_iter()
        .map(|(path, text)| (path.to_string(), source_file(path, text)))
        .collect();
    let preflight = preflight_corpus_symbols(&files, &SymbolPreflightOptions::default())
        .expect("language census");
    preflight
        .admit(SymbolCoveragePolicy::RequireComplete)
        .expect("all grammars complete");
    assert_eq!(preflight.report().admitted_files, 6);
    for file in &preflight.report().files {
        assert_eq!(file.coverage, SymbolCoverage::Complete { symbol_count: 1 });
        let symbols = preflight.symbols().get(&file.path).expect("source symbols");
        assert_eq!(
            symbols.first().expect("sentinel").local_name.as_ref(),
            "sentinel"
        );
    }
}

#[test]
fn preflight_diagnostic_ranges_are_sorted_and_bound_to_source_bytes() {
    let file = source_file("broken.ts", "const x = ;\nfunction broken( {\n");
    let preflight = preflight_corpus_symbols(
        &BTreeMap::from([("broken.ts".to_string(), file.clone())]),
        &SymbolPreflightOptions::default(),
    )
    .expect("malformed report");
    let report = preflight.report().files.first().expect("one file");
    assert_eq!(report.coverage, SymbolCoverage::ParseFailed);
    assert!(!report.diagnostics.is_empty());
    let ranges: Vec<_> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let start = diagnostic.byte_start.expect("syntax start");
            let end = diagnostic.byte_end.expect("syntax end");
            assert!(start <= end && end <= file.bytes.len());
            assert!(file.text.is_char_boundary(start) && file.text.is_char_boundary(end));
            (start, end, diagnostic.kind)
        })
        .collect();
    assert!(ranges.windows(2).all(|pair| pair.first() <= pair.last()));
}
