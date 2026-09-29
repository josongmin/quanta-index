#![expect(
    clippy::indexing_slicing,
    reason = "authority tests assert exact match cardinality before direct indexing"
)]

use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    LqMetaVar, LqStructuralBlock, LqStructuralConstraint, LqStructuralConstraintOperand,
    LqStructuralExpr, LqStructuralHoleMultiplicity, LqStructuralHoleRef, LqStructuralNode,
    MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1,
};
use quanta_index_lq_structural::{
    ByteSpan, MetaVar, PreparedStructuralRegexes, StructuralAuthorityMatcher,
    StructuralAuthorityPatternRef, StructuralAuthorityView, StructuralErrorCode, StructuralPattern,
    TruthfulSubsetAuthorityMatcher, compile_authoritative_pattern,
};

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

fn span(a: u32, b: u32) -> ByteSpan {
    match ByteSpan::new(a, b) {
        Ok(s) => s,
        Err(e) => fatal(&format!("{e}")),
    }
}

fn mv(name: &str) -> MetaVar {
    match MetaVar::new(name) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    }
}

fn literal(text: &str) -> LqStructuralNode {
    LqStructuralNode::Literal(text.to_string().into_boxed_str())
}

fn metavar(name: &str) -> LqStructuralNode {
    LqStructuralNode::Hole {
        name: Some(LqMetaVar::new(name.to_string())),
        multiplicity: LqStructuralHoleMultiplicity::One,
    }
}

fn hole_many(name: &str) -> LqStructuralNode {
    LqStructuralNode::Hole {
        name: Some(LqMetaVar::new(name.to_string())),
        multiplicity: LqStructuralHoleMultiplicity::Many,
    }
}

fn typed_hole(name: &str, kind: &str) -> LqStructuralNode {
    LqStructuralNode::Hole {
        name: Some(LqMetaVar::new(format!("{name}.{kind}"))),
        multiplicity: LqStructuralHoleMultiplicity::One,
    }
}

fn wildcard_many() -> LqStructuralNode {
    LqStructuralNode::WildcardMany
}

fn group(children: Vec<LqStructuralNode>) -> LqStructuralNode {
    LqStructuralNode::Group(children)
}

fn structural_block(nodes: Vec<LqStructuralNode>) -> LqStructuralBlock {
    let pattern_nodes = nodes.clone();
    LqStructuralBlock {
        lang: None,
        nodes,
        exprs: vec![LqStructuralExpr::Pattern(pattern_nodes)],
    }
}

fn compile_exprs(exprs: Vec<LqStructuralExpr>, lang: &str) -> StructuralPattern {
    let nodes = match exprs.first() {
        Some(LqStructuralExpr::Pattern(nodes)) => nodes.clone(),
        Some(_) => fatal("first structural expr must be pattern"),
        None => fatal("structural expr list must not be empty"),
    };
    match compile_authoritative_pattern(
        &LqStructuralBlock {
            lang: None,
            nodes,
            exprs,
        },
        lang,
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    }
}

fn compile_block(nodes: Vec<LqStructuralNode>, lang: &str) -> StructuralPattern {
    let block = structural_block(nodes);
    compile_exprs(block.exprs, lang)
}

fn hole_ref(name: &str, multiplicity: LqStructuralHoleMultiplicity) -> LqStructuralHoleRef {
    LqStructuralHoleRef {
        name: LqMetaVar::new(name.to_string()),
        multiplicity,
    }
}

fn tree_with_children(
    lang: &str,
    kind: &str,
    start: u32,
    end: u32,
    source: &str,
    children: Vec<ParseNode>,
) -> ParseTreeRecord {
    let Ok(lang) = LanguageCode::new(lang) else {
        fatal("language");
    };
    ParseTreeRecord {
        wire_version: 1,
        lang,
        root: ParseNode {
            kind: kind.to_string().into_boxed_str(),
            byte_start: start,
            byte_end: end,
            children,
        },
        source_hash: compute_parse_tree_source_hash(source),
        role_tag_schema_version: 1,
        role_tags: Vec::new(),
    }
}

fn tree_with_children_and_roles(
    lang: &str,
    kind: &str,
    start: u32,
    end: u32,
    source: &str,
    children: Vec<ParseNode>,
    role_tags: Vec<ParseRoleTag>,
) -> ParseTreeRecord {
    let mut tree = tree_with_children(lang, kind, start, end, source, children);
    tree.role_tags = role_tags;
    tree
}

fn tree(lang: &str, kind: &str, start: u32, end: u32, source: &str) -> ParseTreeRecord {
    tree_with_children(lang, kind, start, end, source, Vec::new())
}

fn lower(pattern: &StructuralPattern) -> StructuralAuthorityPatternRef<'_> {
    match StructuralAuthorityPatternRef::try_from(pattern) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    }
}

#[test]
fn authority_root_kind_exact_match_returns_one_candidate() {
    let source = "fn main() {}";
    let tree = tree("rust", "function_item", 0, 12, source);
    let pattern = compile_block(vec![literal(" function_item ")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pattern_span, span(0, 12));
    assert!(got[0].binding.is_empty());
}

#[test]
fn compile_authoritative_pattern_trims_single_root_literal() {
    let pattern = compile_block(vec![literal("  function_item  ")], "rust");
    let lowered = lower(&pattern);
    assert_eq!(
        lowered.kind(),
        quanta_index_lq_structural::StructuralAuthorityPatternKind::RootKind("function_item")
    );
}

#[test]
fn authority_root_kind_exact_miss_returns_empty() {
    let source = "fn main() {}";
    let tree = tree("rust", "function_item", 0, 12, source);
    let pattern = compile_block(vec![literal("identifier")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert!(got.is_empty());
}

#[test]
fn authority_group_wrapped_root_capture_binds_root_span() {
    let source = "fn main() {}";
    let tree = tree("rust", "function_item", 0, 12, source);
    let pattern = compile_block(
        vec![group(vec![literal(" "), metavar("node"), literal(" ")])],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pattern_span, span(0, 12));
    assert_eq!(got[0].binding.get(&mv("node")), Some(span(0, 12)));
}

#[test]
fn authority_root_kind_plus_capture_binds_root_span() {
    let source = "fn main() {}";
    let tree = tree("rust", "function_item", 0, 12, source);
    let pattern = compile_block(vec![literal("function_item"), metavar("node")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pattern_span, span(0, 12));
    assert_eq!(got[0].binding.get(&mv("node")), Some(span(0, 12)));
}

#[test]
fn authority_tree_walk_binds_direct_child_capture() {
    let source = "fn main() {}";
    let tree = tree_with_children(
        "rust",
        "function_item",
        0,
        10,
        source,
        vec![
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 3,
                byte_end: 7,
                children: Vec::new(),
            },
            ParseNode {
                kind: "block".to_string().into_boxed_str(),
                byte_start: 8,
                byte_end: 10,
                children: Vec::new(),
            },
        ],
    );
    let pattern = compile_block(
        vec![
            literal("function_item"),
            group(vec![group(vec![literal("identifier"), metavar("name")])]),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pattern_span, span(0, 10));
    assert_eq!(got[0].binding.get(&mv("name")), Some(span(3, 7)));
}

#[test]
fn authority_typed_expr_hole_matches_role_tag_exact_span() {
    let source = "fn main() {}";
    let tree = tree_with_children_and_roles(
        "rust",
        "function_item",
        0,
        10,
        source,
        vec![
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 3,
                byte_end: 7,
                children: Vec::new(),
            },
            ParseNode {
                kind: "block".to_string().into_boxed_str(),
                byte_start: 8,
                byte_end: 10,
                children: Vec::new(),
            },
        ],
        vec![
            ParseRoleTag {
                role: "item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 10,
            },
            ParseRoleTag {
                role: "expr".to_string().into_boxed_str(),
                byte_start: 3,
                byte_end: 7,
            },
            ParseRoleTag {
                role: "stmt".to_string().into_boxed_str(),
                byte_start: 8,
                byte_end: 10,
            },
        ],
    );
    let pattern = compile_block(
        vec![
            literal("function_item"),
            group(vec![group(vec![typed_hole("name", "expr")])]),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].binding.get(&mv("name")), Some(span(3, 7)));
}

#[test]
fn authority_typed_item_hole_matches_root_role_tag() {
    let source = "fn main() {}";
    let tree = tree_with_children_and_roles(
        "rust",
        "function_item",
        0,
        10,
        source,
        Vec::new(),
        vec![ParseRoleTag {
            role: "item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 10,
        }],
    );
    let pattern = compile_block(vec![typed_hole("root", "item")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].binding.get(&mv("root")), Some(span(0, 10)));
}

#[test]
fn authority_typed_stmt_hole_matches_stmt_role_tag() {
    let source = "fn main() {}";
    let tree = tree_with_children_and_roles(
        "rust",
        "function_item",
        0,
        10,
        source,
        vec![ParseNode {
            kind: "block".to_string().into_boxed_str(),
            byte_start: 8,
            byte_end: 10,
            children: Vec::new(),
        }],
        vec![ParseRoleTag {
            role: "stmt".to_string().into_boxed_str(),
            byte_start: 8,
            byte_end: 10,
        }],
    );
    let pattern = compile_block(
        vec![
            literal("function_item"),
            group(vec![typed_hole("stmt_node", "stmt")]),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].binding.get(&mv("stmt_node")), Some(span(8, 10)));
}

#[test]
fn authority_typed_type_hole_matches_type_role_tag() {
    let source = "T";
    let tree = tree_with_children_and_roles(
        "rust",
        "type_identifier",
        0,
        1,
        source,
        Vec::new(),
        vec![ParseRoleTag {
            role: "type".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 1,
        }],
    );
    let pattern = compile_block(vec![typed_hole("type_node", "type")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].binding.get(&mv("type_node")), Some(span(0, 1)));
}

#[test]
fn authority_typed_hole_unknown_kind_fails_closed() {
    let err =
        compile_authoritative_pattern(&structural_block(vec![typed_hole("x", "lambda")]), "rust");
    match err {
        Ok(pattern) => fatal(&format!("expected typed-hole failure, got {pattern:?}")),
        Err(err) => assert_eq!(err.code, StructuralErrorCode::StrHoleKindUnsupported),
    }
}

#[test]
fn authority_tree_walk_variadic_tail_capture_binds_remaining_siblings() {
    let source = "fn main() {}";
    let tree = tree_with_children(
        "rust",
        "function_item",
        0,
        10,
        source,
        vec![
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 3,
                byte_end: 7,
                children: Vec::new(),
            },
            ParseNode {
                kind: "block".to_string().into_boxed_str(),
                byte_start: 8,
                byte_end: 10,
                children: Vec::new(),
            },
        ],
    );
    let pattern = compile_block(
        vec![
            literal("function_item"),
            group(vec![literal("identifier"), metavar("name")]),
            hole_many("tail"),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pattern_span, span(0, 10));
    assert_eq!(got[0].binding.get(&mv("name")), Some(span(3, 7)));
    assert_eq!(got[0].binding.get(&mv("tail")), Some(span(8, 10)));
}

#[test]
fn authority_where_constraint_filters_by_bound_source_text() {
    let equal_source = "aa";
    let unequal_source = "ab";
    let equal_tree = tree_with_children(
        "rust",
        "pair",
        0,
        2,
        equal_source,
        vec![
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 1,
                children: Vec::new(),
            },
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 1,
                byte_end: 2,
                children: Vec::new(),
            },
        ],
    );
    let unequal_tree = tree_with_children(
        "rust",
        "pair",
        0,
        2,
        unequal_source,
        vec![
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 1,
                children: Vec::new(),
            },
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 1,
                byte_end: 2,
                children: Vec::new(),
            },
        ],
    );
    let pattern = compile_exprs(
        vec![
            LqStructuralExpr::Pattern(vec![
                literal("pair"),
                group(vec![literal("identifier"), metavar("lhs")]),
                group(vec![literal("identifier"), metavar("rhs")]),
            ]),
            LqStructuralExpr::Where(vec![LqStructuralConstraint {
                left: hole_ref("lhs", LqStructuralHoleMultiplicity::One),
                right: LqStructuralConstraintOperand::Hole(hole_ref(
                    "rhs",
                    LqStructuralHoleMultiplicity::One,
                )),
            }]),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let equal = match matcher.match_authority(
        lower(&pattern),
        StructuralAuthorityView::new(equal_source, &equal_tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    let unequal = match matcher.match_authority(
        lower(&pattern),
        StructuralAuthorityView::new(unequal_source, &unequal_tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(equal.len(), 1);
    assert!(unequal.is_empty());
}

#[test]
fn authority_where_regex_constraint_filters_by_bound_source_text() {
    let matching_source = "alpha";
    let non_matching_source = "beta";
    let matching_end = match u32::try_from(matching_source.len()) {
        Ok(value) => value,
        Err(err) => fatal(&format!("test source length must fit u32: {err}")),
    };
    let non_matching_end = match u32::try_from(non_matching_source.len()) {
        Ok(value) => value,
        Err(err) => fatal(&format!("test source length must fit u32: {err}")),
    };
    let matching_tree = tree("rust", "identifier", 0, matching_end, matching_source);
    let non_matching_tree = tree(
        "rust",
        "identifier",
        0,
        non_matching_end,
        non_matching_source,
    );
    let exprs = vec![
        LqStructuralExpr::Pattern(vec![metavar("name")]),
        LqStructuralExpr::Where(vec![LqStructuralConstraint {
            left: hole_ref("name", LqStructuralHoleMultiplicity::One),
            right: LqStructuralConstraintOperand::Regex("^alpha$".to_string()),
        }]),
    ];
    let pattern = compile_exprs(exprs.clone(), "rust");
    let prepared = match PreparedStructuralRegexes::from_block(&LqStructuralBlock {
        lang: None,
        nodes: vec![metavar("name")],
        exprs,
    }) {
        Ok(prepared) => prepared,
        Err(error) => fatal(&format!("valid regex admission failed: {error}")),
    };
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let matching = match matcher.match_authority(
        lower(&pattern),
        StructuralAuthorityView::new(matching_source, &matching_tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    let non_matching = match matcher.match_authority(
        lower(&pattern),
        StructuralAuthorityView::new(non_matching_source, &non_matching_tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(matching.len(), 1);
    assert!(non_matching.is_empty());
    let prepared_matching = match matcher.match_authority_prepared(
        lower(&pattern),
        StructuralAuthorityView::new(matching_source, &matching_tree),
        &prepared,
    ) {
        Ok(candidates) => candidates,
        Err(error) => fatal(&format!("prepared regex match failed: {error}")),
    };
    let prepared_non_matching = match matcher.match_authority_prepared(
        lower(&pattern),
        StructuralAuthorityView::new(non_matching_source, &non_matching_tree),
        &prepared,
    ) {
        Ok(candidates) => candidates,
        Err(error) => fatal(&format!("prepared regex match failed: {error}")),
    };
    assert_eq!(prepared_matching, matching);
    assert_eq!(prepared_non_matching, non_matching);
}

#[test]
fn authority_where_regex_distinguishes_input_and_resource_refusal() {
    let source = "alpha";
    let authority = tree("rust", "identifier", 0, 5, source);
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    for (regex, expected) in [
        ("[".to_string(), StructuralErrorCode::RegexInvalidPattern),
        (
            "x".repeat(65_537),
            StructuralErrorCode::RegexPlanLimitExceeded,
        ),
        (
            r"[\x{80}-\x{10FFFF}]{20000}".to_string(),
            StructuralErrorCode::RegexPlanLimitExceeded,
        ),
    ] {
        let pattern = compile_exprs(
            vec![
                LqStructuralExpr::Pattern(vec![metavar("name")]),
                LqStructuralExpr::Where(vec![LqStructuralConstraint {
                    left: hole_ref("name", LqStructuralHoleMultiplicity::One),
                    right: LqStructuralConstraintOperand::Regex(regex),
                }]),
            ],
            "rust",
        );
        let error = matcher
            .match_authority(
                lower(&pattern),
                StructuralAuthorityView::new(source, &authority),
            )
            .expect_err("invalid regex must refuse rather than act as a non-match");
        assert_eq!(error.code, expected);
    }
}

#[test]
fn structural_where_regex_engine_count_is_bounded_by_distinct_patterns() {
    let block = |patterns: Vec<String>| LqStructuralBlock {
        lang: None,
        nodes: vec![metavar("name")],
        exprs: vec![
            LqStructuralExpr::Pattern(vec![metavar("name")]),
            LqStructuralExpr::Where(
                patterns
                    .into_iter()
                    .map(|pattern| LqStructuralConstraint {
                        left: hole_ref("name", LqStructuralHoleMultiplicity::One),
                        right: LqStructuralConstraintOperand::Regex(pattern),
                    })
                    .collect(),
            ),
        ],
    };
    let mut patterns = (0..MAX_STRUCTURAL_WHERE_REGEX_ENGINES_V1)
        .map(|index| format!("^value{index}$"))
        .collect::<Vec<_>>();
    patterns.push(patterns[0].clone());
    assert!(PreparedStructuralRegexes::from_block(&block(patterns.clone())).is_ok());
    patterns.push("^one_more$".to_owned());
    match PreparedStructuralRegexes::from_block(&block(patterns)) {
        Err(error) => assert_eq!(error.code, StructuralErrorCode::RegexPlanLimitExceeded),
        Ok(_) => fatal("ninth distinct structural regex must be refused"),
    }
}

#[test]
fn authority_inside_and_outside_constraints_follow_ancestor_chain() {
    let nested_source = "impl x";
    let nested_tree = tree_with_children(
        "rust",
        "impl_item",
        0,
        6,
        nested_source,
        vec![ParseNode {
            kind: "function_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 6,
            children: Vec::new(),
        }],
    );
    let standalone_source = "fn x";
    let standalone_tree = tree("rust", "function_item", 0, 4, standalone_source);
    let trait_source = "trait x";
    let trait_tree = tree_with_children(
        "rust",
        "trait_item",
        0,
        7,
        trait_source,
        vec![ParseNode {
            kind: "function_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 7,
            children: Vec::new(),
        }],
    );
    let pattern = compile_exprs(
        vec![
            LqStructuralExpr::Pattern(vec![literal("function_item")]),
            LqStructuralExpr::Inside(Box::new(structural_block(vec![literal("impl_item")]))),
            LqStructuralExpr::Outside(Box::new(structural_block(vec![literal("trait_item")]))),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let nested = match matcher.match_authority(
        lower(&pattern),
        StructuralAuthorityView::new(nested_source, &nested_tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    let standalone = match matcher.match_authority(
        lower(&pattern),
        StructuralAuthorityView::new(standalone_source, &standalone_tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    let trait_nested = match matcher.match_authority(
        lower(&pattern),
        StructuralAuthorityView::new(trait_source, &trait_tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(nested.len(), 1);
    assert!(standalone.is_empty());
    assert!(trait_nested.is_empty());
}

#[test]
fn authority_pattern_lowering_rejects_still_unsupported_composite_shape() {
    let pattern = compile_block(
        vec![
            literal("function_item"),
            metavar("node"),
            literal("identifier"),
        ],
        "rust",
    );
    let lowered = lower(&pattern);
    assert_eq!(
        lowered.kind(),
        quanta_index_lq_structural::StructuralAuthorityPatternKind::Tree
    );
}

#[test]
fn authority_pattern_lowering_rejects_ambiguous_child_sequence_shape() {
    let pattern = compile_block(
        vec![
            literal("function_item"),
            group(vec![group(vec![
                literal("identifier"),
                metavar("name"),
                literal("block"),
            ])]),
        ],
        "rust",
    );
    let lowered = lower(&pattern);
    assert_eq!(
        lowered.kind(),
        quanta_index_lq_structural::StructuralAuthorityPatternKind::Tree
    );
}

#[test]
fn compile_authoritative_pattern_lowers_root_kind_plus_capture() {
    let pattern = compile_block(vec![literal(" function_item "), metavar("node")], "rust");
    let lowered = lower(&pattern);
    assert_eq!(
        lowered.kind(),
        quanta_index_lq_structural::StructuralAuthorityPatternKind::RootKindCapture(
            "function_item",
            &mv("node"),
        )
    );
}

#[test]
fn authority_match_rejects_unsupported_tree_lang() {
    let source = "class C {}";
    let tree = tree("java", "class_declaration", 0, 10, source);
    let pattern = compile_block(vec![literal("class_declaration")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    match matcher.match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree)) {
        Ok(_) => fatal("must fail closed"),
        Err(err) => assert_eq!(err.code, StructuralErrorCode::StrLangNotSupported),
    }
}

#[test]
fn authority_match_rejects_source_hash_mismatch() {
    let source = "fn main() {}";
    let mut tree = tree("rust", "function_item", 0, 12, source);
    tree.source_hash = compute_parse_tree_source_hash("fn other() {}");
    let pattern = compile_block(vec![literal("function_item")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    match matcher.match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree)) {
        Ok(_) => fatal("must fail closed"),
        Err(err) => assert_eq!(err.code, StructuralErrorCode::StrParseFail),
    }
}

#[test]
fn authority_match_rejects_root_span_out_of_bounds() {
    let source = "abc";
    let tree = tree("rust", "identifier", 0, 10, source);
    let pattern = compile_block(vec![literal("identifier")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    match matcher.match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree)) {
        Ok(_) => fatal("must fail closed"),
        Err(err) => assert_eq!(err.code, StructuralErrorCode::StrParseFail),
    }
}

#[test]
fn authority_root_fast_path_does_not_descend_to_children() {
    let source = "fn main() {}";
    let tree = tree_with_children(
        "rust",
        "function_item",
        0,
        12,
        source,
        vec![ParseNode {
            kind: "identifier".to_string().into_boxed_str(),
            byte_start: 3,
            byte_end: 7,
            children: Vec::new(),
        }],
    );
    let pattern = compile_block(vec![literal("identifier")], "rust");
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert!(got.is_empty());
}

#[test]
fn authority_tree_walk_binds_variadic_contiguous_span() {
    let source = "fn main(arg1, arg2) {}";
    let tree = tree_with_children(
        "rust",
        "function_item",
        0,
        22,
        source,
        vec![
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 3,
                byte_end: 7,
                children: Vec::new(),
            },
            ParseNode {
                kind: "parameters".to_string().into_boxed_str(),
                byte_start: 7,
                byte_end: 19,
                children: Vec::new(),
            },
            ParseNode {
                kind: "block".to_string().into_boxed_str(),
                byte_start: 20,
                byte_end: 22,
                children: Vec::new(),
            },
        ],
    );
    let pattern = compile_block(
        vec![
            literal("function_item"),
            group(vec![hole_many("prefix"), literal("block")]),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].binding.get(&mv("prefix")), Some(span(3, 19)));
}

#[test]
fn authority_tree_walk_applies_where_inside_and_outside() {
    let source = "fn main() {}";
    let tree = tree_with_children(
        "rust",
        "function_item",
        0,
        12,
        source,
        vec![ParseNode {
            kind: "identifier".to_string().into_boxed_str(),
            byte_start: 3,
            byte_end: 7,
            children: Vec::new(),
        }],
    );
    let pattern = match compile_authoritative_pattern(
        &LqStructuralBlock {
            lang: None,
            nodes: Vec::new(),
            exprs: vec![
                LqStructuralExpr::Pattern(vec![literal("identifier"), metavar("name")]),
                LqStructuralExpr::Where(vec![LqStructuralConstraint {
                    left: LqStructuralHoleRef {
                        name: LqMetaVar::new("name".to_string()),
                        multiplicity: LqStructuralHoleMultiplicity::One,
                    },
                    right: LqStructuralConstraintOperand::Phrase("main".to_string()),
                }]),
                LqStructuralExpr::Inside(Box::new(LqStructuralBlock {
                    lang: None,
                    nodes: Vec::new(),
                    exprs: vec![LqStructuralExpr::Pattern(vec![literal("function_item")])],
                })),
                LqStructuralExpr::Outside(Box::new(LqStructuralBlock {
                    lang: None,
                    nodes: Vec::new(),
                    exprs: vec![LqStructuralExpr::Pattern(vec![literal("trait_item")])],
                })),
            ],
        },
        "rust",
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pattern_span, span(3, 7));
    assert_eq!(got[0].binding.get(&mv("name")), Some(span(3, 7)));
}

#[test]
fn authority_wildcard_many_can_skip_to_anchor() {
    let source = "fn main() {}";
    let tree = tree_with_children(
        "rust",
        "function_item",
        0,
        12,
        source,
        vec![
            ParseNode {
                kind: "identifier".to_string().into_boxed_str(),
                byte_start: 3,
                byte_end: 7,
                children: Vec::new(),
            },
            ParseNode {
                kind: "block".to_string().into_boxed_str(),
                byte_start: 10,
                byte_end: 12,
                children: Vec::new(),
            },
        ],
    );
    let pattern = compile_block(
        vec![
            literal("function_item"),
            group(vec![wildcard_many(), literal("block")]),
        ],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher
        .match_authority(lower(&pattern), StructuralAuthorityView::new(source, &tree))
    {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pattern_span, span(0, 12));
}
