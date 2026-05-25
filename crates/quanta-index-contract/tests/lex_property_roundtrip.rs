//! Property tests: every `lex::*` record round-trips through CBOR
//! identically. 256 cases per type (proptest default).
//!
//! Per `CLAUDE.md` D18 + workspace lints we hand-roll the `Strategy`
//! definitions rather than relying on `proptest-derive` (which is also a
//! proc-macro path). Each type below has a `prop_*` strategy that produces
//! deterministic-shrinking values; we feed those into a single closure that
//! asserts `decode(encode(x)) == x`.

#![forbid(unsafe_code)]

use proptest::collection::vec as prop_vec;
use proptest::prelude::*;
use quanta_index_contract::ChunkId;
use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DirtyRecord, ExplanationRow, LangId, LexicalErrorCode, ParseNode,
    ParseTreeRecord, PlannerTraceNode, SearchExplanation, SymbolKind, SymbolRecord,
    SymbolRelationship, SymbolSpan,
};

fn cbor_roundtrip<T>(value: &T) -> Result<T, String>
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de>,
{
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(value, &mut buf).map_err(|err| err.to_string())?;
    ciborium::de::from_reader(buf.as_slice()).map_err(|err| err.to_string())
}

// ---- strategies ----------------------------------------------------------

fn prop_lang_id() -> impl Strategy<Value = LangId> {
    prop_oneof![
        Just(LangId::Rust),
        Just(LangId::Python),
        Just(LangId::TypeScript),
        Just(LangId::JavaScript),
        Just(LangId::Go),
    ]
}

fn prop_symbol_kind() -> impl Strategy<Value = SymbolKind> {
    prop_oneof![
        Just(SymbolKind::Function),
        Just(SymbolKind::Method),
        Just(SymbolKind::Class),
        Just(SymbolKind::Struct),
        Just(SymbolKind::Enum),
        Just(SymbolKind::Trait),
        Just(SymbolKind::Interface),
        Just(SymbolKind::Variable),
        Just(SymbolKind::Constant),
        Just(SymbolKind::Module),
        Just(SymbolKind::Macro),
        Just(SymbolKind::TypeAlias),
    ]
}

fn prop_symbol_relationship() -> impl Strategy<Value = SymbolRelationship> {
    prop_oneof![Just(SymbolRelationship::Def), Just(SymbolRelationship::Ref)]
}

fn prop_lexical_error_code() -> impl Strategy<Value = LexicalErrorCode> {
    // Sample one of the 84 variants by index.
    (0usize..LexicalErrorCode::ALL.len()).prop_map(|idx| {
        LexicalErrorCode::ALL
            .get(idx)
            .copied()
            .unwrap_or(LexicalErrorCode::SyntaxError)
    })
}

fn prop_commit_sha() -> impl Strategy<Value = CommitSha> {
    any::<[u8; 20]>().prop_map(CommitSha::from_bytes)
}

fn prop_symbol_span() -> impl Strategy<Value = SymbolSpan> {
    (
        ".{0,32}",
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
    )
        .prop_map(
            |(path, byte_start, byte_end, line_start, line_end)| SymbolSpan {
                path: path.into_boxed_str(),
                byte_start,
                byte_end,
                line_start,
                line_end,
            },
        )
}

fn prop_symbol_record() -> impl Strategy<Value = SymbolRecord> {
    (
        any::<u32>(),
        ".{0,32}",
        prop_symbol_kind(),
        prop_symbol_span(),
        prop_lang_id(),
        proptest::option::of(".{0,32}"),
        proptest::option::of(".{0,32}"),
        prop_symbol_relationship(),
    )
        .prop_map(
            |(wire_version, name, kind, span, lang, parent, container_name, relationship)| {
                SymbolRecord {
                    wire_version,
                    name: name.into_boxed_str(),
                    kind,
                    span,
                    lang,
                    parent: parent.map(String::into_boxed_str),
                    container_name: container_name.map(String::into_boxed_str),
                    relationship,
                }
            },
        )
}

fn prop_commit_record() -> impl Strategy<Value = CommitRecord> {
    (
        any::<u32>(),
        prop_commit_sha(),
        prop_vec(prop_commit_sha(), 0..4),
        // QI-LXB-01: triple of author / committer / applied timestamps.
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        ".{0,32}",
        ".{0,32}",
        ".{0,32}",
        any::<bool>(),
        prop_vec(".{0,16}", 0..3),
    )
        .prop_map(
            |(
                wire_version,
                sha,
                parents,
                author_time_ms,
                committer_time_ms,
                applied_at_ms,
                author,
                committer,
                message,
                is_merge,
                tags,
            )| CommitRecord {
                wire_version,
                sha,
                parents,
                author_time_ms,
                committer_time_ms,
                applied_at_ms,
                author: author.into_boxed_str(),
                committer: committer.into_boxed_str(),
                message: message.into_boxed_str(),
                is_merge,
                tags: tags.into_iter().map(String::into_boxed_str).collect(),
            },
        )
}

fn prop_dirty_record() -> impl Strategy<Value = DirtyRecord> {
    (any::<u32>(), ".{0,16}", any::<u64>(), any::<[u8; 32]>()).prop_map(
        |(wire_version, doc_id, applied_at_ms, payload_hash)| DirtyRecord {
            wire_version,
            doc_id: ChunkId::new(doc_id),
            applied_at_ms,
            payload_hash,
        },
    )
}

fn prop_parse_node() -> impl Strategy<Value = ParseNode> {
    // Bounded recursive node. Leaves at depth 0; up to 3 children at each
    // level; total depth capped at 3 to keep CBOR encode work small (256
    // cases x recursive trees would blow up otherwise).
    let leaf = (".{1,16}", any::<u32>(), any::<u32>()).prop_map(|(kind, byte_start, byte_end)| {
        ParseNode {
            kind: kind.into_boxed_str(),
            byte_start,
            byte_end,
            children: Vec::new(),
        }
    });
    leaf.prop_recursive(3, 16, 3, |inner| {
        (".{1,16}", any::<u32>(), any::<u32>(), prop_vec(inner, 0..3)).prop_map(
            |(kind, byte_start, byte_end, children)| ParseNode {
                kind: kind.into_boxed_str(),
                byte_start,
                byte_end,
                children,
            },
        )
    })
}

fn prop_parse_tree_record() -> impl Strategy<Value = ParseTreeRecord> {
    (
        any::<u32>(),
        prop_lang_id(),
        prop_parse_node(),
        any::<[u8; 32]>(),
    )
        .prop_map(|(wire_version, lang, root, source_hash)| ParseTreeRecord {
            wire_version,
            lang,
            root,
            source_hash,
            // QI-LXB-01: structural role-tag schema landed alongside
            // ParseTreeRecord. Property tests pass empty tags — separate
            // role-tag-specific tests cover the populated case.
            role_tag_schema_version: 0,
            role_tags: Vec::new(),
        })
}

fn prop_explanation_row() -> impl Strategy<Value = ExplanationRow> {
    // Restrict floats to finite values; NaN does not round-trip equality.
    let finite_f32 = (-1.0e6_f32..1.0e6_f32).prop_filter("finite", |v| v.is_finite());
    (
        ".{0,16}",
        finite_f32.clone(),
        finite_f32.clone(),
        finite_f32,
    )
        .prop_map(
            |(signal_name, signal_value, weight, contribution)| ExplanationRow {
                signal_name: signal_name.into_boxed_str(),
                signal_value,
                weight,
                contribution,
            },
        )
}

fn prop_planner_trace_node() -> impl Strategy<Value = PlannerTraceNode> {
    (".{0,16}", ".{0,16}").prop_map(|(node_kind, detail)| PlannerTraceNode {
        node_kind: node_kind.into_boxed_str(),
        detail: detail.into_boxed_str(),
    })
}

fn prop_search_explanation() -> impl Strategy<Value = SearchExplanation> {
    (
        prop_vec(prop_explanation_row(), 0..4),
        any::<[u8; 32]>(),
        ".{0,16}",
        prop_vec(prop_planner_trace_node(), 0..3),
        prop_vec(".{0,16}", 0..3),
        proptest::option::of(".{0,16}"),
        proptest::option::of(".{0,16}"),
    )
        .prop_map(
            |(
                contributions,
                ranker_weights_hash,
                strategy,
                planner_trace,
                engines_touched,
                early_stop_reason,
                summary,
            )| SearchExplanation {
                contributions,
                ranker_weights_hash,
                strategy: strategy.into_boxed_str(),
                planner_trace,
                engines_touched: engines_touched
                    .into_iter()
                    .map(String::into_boxed_str)
                    .collect(),
                early_stop_reason: early_stop_reason.map(String::into_boxed_str),
                summary: summary.map(String::into_boxed_str),
            },
        )
}

// ---- proptest tests -----------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn lang_id_cbor_roundtrip_property(lang in prop_lang_id()) {
        let decoded = cbor_roundtrip(&lang).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, lang);
    }

    #[test]
    fn symbol_kind_cbor_roundtrip_property(kind in prop_symbol_kind()) {
        let decoded = cbor_roundtrip(&kind).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, kind);
    }

    #[test]
    fn symbol_relationship_cbor_roundtrip_property(rel in prop_symbol_relationship()) {
        let decoded = cbor_roundtrip(&rel).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, rel);
    }

    #[test]
    fn lexical_error_code_cbor_roundtrip_property(code in prop_lexical_error_code()) {
        let decoded = cbor_roundtrip(&code).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, code);
    }

    #[test]
    fn commit_sha_cbor_roundtrip_property(sha in prop_commit_sha()) {
        let decoded = cbor_roundtrip(&sha).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, sha);
    }

    #[test]
    fn symbol_span_cbor_roundtrip_property(span in prop_symbol_span()) {
        let decoded = cbor_roundtrip(&span).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, span);
    }

    #[test]
    fn symbol_record_cbor_roundtrip_property(rec in prop_symbol_record()) {
        let decoded = cbor_roundtrip(&rec).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, rec);
    }

    #[test]
    fn commit_record_cbor_roundtrip_property(rec in prop_commit_record()) {
        let decoded = cbor_roundtrip(&rec).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, rec);
    }

    #[test]
    fn dirty_record_cbor_roundtrip_property(rec in prop_dirty_record()) {
        let decoded = cbor_roundtrip(&rec).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, rec);
    }

    #[test]
    fn parse_tree_record_cbor_roundtrip_property(rec in prop_parse_tree_record()) {
        let decoded = cbor_roundtrip(&rec).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, rec);
    }

    #[test]
    fn search_explanation_cbor_roundtrip_property(rec in prop_search_explanation()) {
        let decoded = cbor_roundtrip(&rec).map_err(TestCaseError::fail)?;
        prop_assert_eq!(decoded, rec);
    }
}
