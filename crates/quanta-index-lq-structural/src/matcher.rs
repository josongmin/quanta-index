//! Structural parse-tree-authority matcher surface and implementations.
//!
//! [`StructuralAuthorityMatcher`] consumes already-materialized
//! [`quanta_index_contract::lex::ParseTreeRecord`] plus chunk text authority
//! and never reparses source.
//!
//! The first truthful authority implementation is
//! [`TruthfulSubsetAuthorityMatcher`]. It executes the currently honest
//! root-anchored subset: root-kind exact, single root capture, root-kind plus
//! root capture, and ordered direct-child tree-walk over producer parse-tree
//! structure.
//!
//! D18 — no proc-macro derives.

use core::fmt;

use quanta_index_contract::{
    LqStructuralBlock, LqStructuralConstraint, LqStructuralConstraintOperand, LqStructuralExpr,
    LqStructuralHoleMultiplicity, LqStructuralHoleRef, LqStructuralNode,
    lex::{ParseNode, ParseTreeRecord, compute_parse_tree_source_hash},
};

use crate::binding::{StructuralAuthorityCandidate, StructuralBinding};
use crate::errors::{StructuralError, StructuralErrorCode};
use crate::pattern::{
    ConstraintOperand, HoleMultiplicity, HoleRef, PatternNode, StructuralConstraint,
    StructuralPattern,
};
use crate::types::{ByteSpan, LangId, MetaVar};

/// Compile a contract-layer structural block into the shared structural IR,
/// anchored to a canonical lowercase producer language code.
pub fn compile_authoritative_pattern(
    block: &LqStructuralBlock,
    lang: &str,
) -> Result<StructuralPattern, StructuralError> {
    let lang = LangId::from_language_code_str(lang)
        .ok_or_else(|| StructuralError::lang_not_supported(lang))?;
    let mut root: Option<PatternNode> = None;
    let mut constraints = Vec::new();
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for expr in &block.exprs {
        match expr {
            LqStructuralExpr::Pattern(nodes) => {
                if root.is_some() {
                    return Err(StructuralError::new(
                        StructuralErrorCode::StrParseFail,
                        "authoritative structural compiler requires exactly one pattern expr",
                    ));
                }
                root = Some(lower_contract_pattern_nodes(nodes)?);
            }
            LqStructuralExpr::Where(items) => {
                constraints.extend(
                    items
                        .iter()
                        .map(lower_contract_constraint)
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            LqStructuralExpr::Inside(block) => {
                inside.push(compile_authoritative_pattern(
                    block,
                    lang.as_language_code_str(),
                )?);
            }
            LqStructuralExpr::Outside(block) => {
                outside.push(compile_authoritative_pattern(
                    block,
                    lang.as_language_code_str(),
                )?);
            }
        }
    }
    let root = root.ok_or_else(|| {
        StructuralError::new(
            StructuralErrorCode::StrParseFail,
            "authoritative structural compiler requires exactly one pattern expr",
        )
    })?;
    StructuralPattern::from_parts(lang, root, constraints, inside, outside)
}

/// Borrowed authoritative input for parse-tree execution.
#[derive(Clone, Copy, Debug)]
pub struct StructuralAuthorityView<'a> {
    /// Chunk text already validated and materialized by the caller.
    pub source: &'a str,
    /// Producer-authored parse tree over the same chunk text authority.
    pub tree: &'a ParseTreeRecord,
}

impl<'a> StructuralAuthorityView<'a> {
    /// Construct a [`StructuralAuthorityView`].
    #[must_use]
    pub const fn new(source: &'a str, tree: &'a ParseTreeRecord) -> Self {
        Self { source, tree }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct StructuralAuthorityNodeShape<'a> {
    expected_kind: Option<&'a str>,
    capture: Option<&'a MetaVar>,
    child_sequence: Option<&'a [PatternNode]>,
}

/// Subset pattern kind accepted by the current authoritative matcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StructuralAuthorityPatternKind<'a> {
    /// Match iff the authority root node kind is exactly `expected_kind`.
    RootKind(&'a str),
    /// Capture the full authority root span into one metavariable.
    RootCapture(&'a MetaVar),
    /// Match the authority root kind exactly and capture the full root span.
    RootKindCapture(&'a str, &'a MetaVar),
    /// Match a root-anchored tree pattern with ordered direct-child walk.
    Tree,
}

/// Lowered authoritative pattern ref. This is intentionally narrower than
/// [`StructuralPattern`] so unsupported shapes fail before execution rather
/// than degrading to heuristic behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructuralAuthorityPatternRef<'a> {
    lang: LangId,
    kind: StructuralAuthorityPatternKind<'a>,
    pattern: &'a StructuralPattern,
}

impl<'a> StructuralAuthorityPatternRef<'a> {
    /// Construct directly from the already-lowered subset shape.
    #[must_use]
    pub const fn new(
        lang: LangId,
        kind: StructuralAuthorityPatternKind<'a>,
        pattern: &'a StructuralPattern,
    ) -> Self {
        Self {
            lang,
            kind,
            pattern,
        }
    }

    /// Borrow the anchor language.
    #[must_use]
    pub const fn lang(self) -> LangId {
        self.lang
    }

    /// Borrow the narrowed subset kind.
    #[must_use]
    pub const fn kind(self) -> StructuralAuthorityPatternKind<'a> {
        self.kind
    }

    #[must_use]
    const fn pattern(self) -> &'a StructuralPattern {
        self.pattern
    }
}

/// Lowering failure from general [`StructuralPattern`] into the truthful
/// authoritative subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StructuralAuthorityPatternError {
    /// Pattern shape is outside the current root-only truthful subset.
    UnsupportedShape,
}

impl fmt::Display for StructuralAuthorityPatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedShape => f.write_str(
                "unsupported structural authority pattern shape; executable structural authority is limited to parse-tree descendant anchors, contiguous sibling sequences, variadic holes, where constraints, and inside/outside context over producer parse-tree authority",
            ),
        }
    }
}

impl core::error::Error for StructuralAuthorityPatternError {}

impl<'a> TryFrom<&'a StructuralPattern> for StructuralAuthorityPatternRef<'a> {
    type Error = StructuralAuthorityPatternError;

    fn try_from(pattern: &'a StructuralPattern) -> Result<Self, Self::Error> {
        if !pattern_is_executable(pattern) {
            return Err(StructuralAuthorityPatternError::UnsupportedShape);
        }
        let kind = if pattern.constraints().is_empty()
            && pattern.inside().is_empty()
            && pattern.outside().is_empty()
        {
            lower_authority_pattern_kind(pattern.root())
                .unwrap_or(StructuralAuthorityPatternKind::Tree)
        } else {
            StructuralAuthorityPatternKind::Tree
        };
        Ok(Self::new(pattern.lang(), kind, pattern))
    }
}

/// Parse-tree-authority matcher port.
///
/// Implementations consume authoritative chunk text + parse tree that have
/// already been materialized by the caller. Reparsing source is out of scope.
pub trait StructuralAuthorityMatcher: Send + Sync {
    /// Match a previously-lowered authoritative subset pattern against one
    /// authoritative chunk/tree pair.
    fn match_authority(
        &self,
        pattern: StructuralAuthorityPatternRef<'_>,
        authority: StructuralAuthorityView<'_>,
    ) -> Result<Vec<StructuralAuthorityCandidate>, StructuralError>;
}

/// Truthful v1 authority matcher: root-kind exact, single root capture,
/// root-kind plus root capture, and ordered direct-child tree-walk.
#[derive(Clone, Copy, Debug, Default)]
pub struct TruthfulSubsetAuthorityMatcher;

impl TruthfulSubsetAuthorityMatcher {
    /// Construct a matcher for the bounded truthful subset.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl StructuralAuthorityMatcher for TruthfulSubsetAuthorityMatcher {
    fn match_authority(
        &self,
        pattern: StructuralAuthorityPatternRef<'_>,
        authority: StructuralAuthorityView<'_>,
    ) -> Result<Vec<StructuralAuthorityCandidate>, StructuralError> {
        validate_authority(pattern, authority)?;
        if !matches!(pattern.kind(), StructuralAuthorityPatternKind::Tree) {
            let binding = match_node_pattern(
                pattern.pattern().root(),
                &authority.tree.root,
                StructuralBinding::empty(),
            )?;
            return Ok(match binding {
                Some(binding) => vec![StructuralAuthorityCandidate::new(
                    ByteSpan::new(authority.tree.root.byte_start, authority.tree.root.byte_end)?,
                    binding,
                )],
                None => Vec::new(),
            });
        }
        let mut out = Vec::new();
        let mut ancestors: Vec<&ParseNode> = Vec::new();
        collect_authority_candidates(
            pattern.pattern(),
            &authority.tree.root,
            &mut ancestors,
            authority.source,
            &mut out,
        )?;
        Ok(out)
    }
}

fn lower_authority_pattern_kind(node: &PatternNode) -> Option<StructuralAuthorityPatternKind<'_>> {
    let shape = lower_authority_node_shape(node)?;
    match (shape.expected_kind, shape.capture, shape.child_sequence) {
        (Some(expected_kind), None, None) => {
            Some(StructuralAuthorityPatternKind::RootKind(expected_kind))
        }
        (None, Some(metavar), None) => Some(StructuralAuthorityPatternKind::RootCapture(metavar)),
        (Some(expected_kind), Some(metavar), None) => Some(
            StructuralAuthorityPatternKind::RootKindCapture(expected_kind, metavar),
        ),
        (_, _, Some(_)) => Some(StructuralAuthorityPatternKind::Tree),
        (None, None, None) => None,
    }
}

fn lower_contract_pattern_nodes(
    nodes: &[LqStructuralNode],
) -> Result<PatternNode, StructuralError> {
    let significant: Vec<&LqStructuralNode> = nodes
        .iter()
        .filter(|node| match node {
            LqStructuralNode::Literal(text) => !text.trim().is_empty(),
            LqStructuralNode::MetaVar(_)
            | LqStructuralNode::Group(_)
            | LqStructuralNode::Hole { .. }
            | LqStructuralNode::WildcardMany => true,
        })
        .collect();
    Ok(match significant.as_slice() {
        [LqStructuralNode::Literal(text)] => {
            PatternNode::Literal(text.trim().to_string().into_boxed_str())
        }
        [node] => lower_contract_pattern_node(node)?,
        _ => PatternNode::Group(
            significant
                .into_iter()
                .map(lower_contract_pattern_node)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

fn lower_contract_pattern_node(node: &LqStructuralNode) -> Result<PatternNode, StructuralError> {
    match node {
        LqStructuralNode::Literal(text) => Ok(PatternNode::Literal(text.clone())),
        LqStructuralNode::MetaVar(metavar) => {
            Ok(PatternNode::Metavar(MetaVar::new(metavar.as_str())?))
        }
        LqStructuralNode::Hole {
            name: Some(metavar),
            multiplicity: LqStructuralHoleMultiplicity::One,
        } => Ok(PatternNode::Metavar(MetaVar::new(metavar.as_str())?)),
        LqStructuralNode::Hole {
            name: Some(metavar),
            multiplicity: LqStructuralHoleMultiplicity::Many,
        } => Ok(PatternNode::HoleMany(MetaVar::new(metavar.as_str())?)),
        LqStructuralNode::Hole {
            name: None,
            multiplicity: LqStructuralHoleMultiplicity::Many,
        }
        | LqStructuralNode::WildcardMany => Ok(PatternNode::WildcardMany),
        LqStructuralNode::Hole { .. } => Err(StructuralError::new(
            StructuralErrorCode::StrParseFail,
            "anonymous non-variadic structural holes are outside the current authority subset",
        )),
        LqStructuralNode::Group(children) => Ok(PatternNode::Group(
            children
                .iter()
                .map(lower_contract_pattern_node)
                .collect::<Result<Vec<_>, _>>()?,
        )),
    }
}

fn lower_contract_constraint(
    constraint: &LqStructuralConstraint,
) -> Result<StructuralConstraint, StructuralError> {
    Ok(StructuralConstraint {
        left: lower_contract_hole_ref(&constraint.left)?,
        right: match &constraint.right {
            LqStructuralConstraintOperand::Hole(hole) => {
                ConstraintOperand::Hole(lower_contract_hole_ref(hole)?)
            }
            LqStructuralConstraintOperand::Phrase(text) => ConstraintOperand::Phrase(text.clone()),
            LqStructuralConstraintOperand::RawString(text) => {
                ConstraintOperand::RawString(text.clone())
            }
        },
    })
}

fn lower_contract_hole_ref(hole: &LqStructuralHoleRef) -> Result<HoleRef, StructuralError> {
    Ok(HoleRef {
        name: MetaVar::new(hole.name.as_str())?,
        multiplicity: match hole.multiplicity {
            LqStructuralHoleMultiplicity::One => HoleMultiplicity::One,
            LqStructuralHoleMultiplicity::Many => HoleMultiplicity::Many,
        },
    })
}

fn validate_root_bounds(span: ByteSpan, source_len: usize) -> Result<(), StructuralError> {
    let end = usize::try_from(span.end()).map_err(|err| {
        StructuralError::new(
            StructuralErrorCode::StrParseFail,
            format!("authoritative root span end does not fit usize: {err}"),
        )
    })?;
    if end > source_len {
        return Err(StructuralError::new(
            StructuralErrorCode::StrParseFail,
            format!("authoritative root span {span} exceeds source length {source_len}"),
        ));
    }
    Ok(())
}

fn validate_node_bounds(node: &ParseNode, source_len: usize) -> Result<(), StructuralError> {
    let span = ByteSpan::new(node.byte_start, node.byte_end)?;
    validate_root_bounds(span, source_len)?;
    for child in &node.children {
        validate_node_bounds(child, source_len)?;
    }
    Ok(())
}

fn validate_authority(
    pattern: StructuralAuthorityPatternRef<'_>,
    authority: StructuralAuthorityView<'_>,
) -> Result<(), StructuralError> {
    let Some(tree_lang) = LangId::from_language_code_str(authority.tree.lang.as_str()) else {
        return Err(StructuralError::lang_not_supported(
            authority.tree.lang.as_str(),
        ));
    };
    if tree_lang != pattern.lang() {
        return Err(StructuralError::new(
            StructuralErrorCode::StrParseFail,
            format!(
                "authoritative tree language {} does not match pattern language {}",
                authority.tree.lang.as_str(),
                pattern.lang().as_language_code_str(),
            ),
        ));
    }
    let expected_hash = compute_parse_tree_source_hash(authority.source);
    if authority.tree.source_hash != expected_hash {
        return Err(StructuralError::new(
            StructuralErrorCode::StrParseFail,
            "authoritative parse-tree source hash mismatch",
        ));
    }
    validate_node_bounds(&authority.tree.root, authority.source.len())
}

fn pattern_is_executable(pattern: &StructuralPattern) -> bool {
    pattern_node_is_executable(pattern.root())
        && pattern.inside().iter().all(pattern_is_executable)
        && pattern.outside().iter().all(pattern_is_executable)
}

fn pattern_node_is_executable(node: &PatternNode) -> bool {
    match node {
        PatternNode::Literal(_)
        | PatternNode::Metavar(_)
        | PatternNode::HoleMany(_)
        | PatternNode::WildcardMany => true,
        PatternNode::Group(children) => children.iter().all(pattern_node_is_executable),
    }
}

fn lower_authority_node_shape(node: &PatternNode) -> Option<StructuralAuthorityNodeShape<'_>> {
    match node {
        PatternNode::Literal(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(StructuralAuthorityNodeShape {
                    expected_kind: Some(trimmed),
                    capture: None,
                    child_sequence: None,
                })
            }
        }
        PatternNode::Metavar(metavar) => Some(StructuralAuthorityNodeShape {
            expected_kind: None,
            capture: Some(metavar),
            child_sequence: None,
        }),
        PatternNode::HoleMany(_) | PatternNode::WildcardMany => {
            Some(StructuralAuthorityNodeShape {
                expected_kind: None,
                capture: None,
                child_sequence: Some(&[]),
            })
        }
        PatternNode::Group(children) => lower_authority_sequence_shape(children),
    }
}

fn lower_authority_sequence_shape(
    children: &[PatternNode],
) -> Option<StructuralAuthorityNodeShape<'_>> {
    let significant = significant_children(children);
    let first = *significant.first()?;
    if significant.len() == 1 {
        return lower_authority_node_shape(first);
    }

    let mut cursor: usize = 0;
    let mut expected_kind: Option<&str> = None;
    let mut capture: Option<&MetaVar> = None;
    let mut child_sequence: Option<&[PatternNode]> = None;

    match significant.get(cursor).copied()? {
        PatternNode::Literal(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return None;
            }
            expected_kind = Some(trimmed);
            cursor = cursor.saturating_add(1);
        }
        PatternNode::Metavar(metavar) => {
            capture = Some(metavar);
            cursor = cursor.saturating_add(1);
        }
        PatternNode::HoleMany(_) | PatternNode::WildcardMany => {
            child_sequence = Some(children);
            cursor = cursor.saturating_add(1);
        }
        PatternNode::Group(_) => return None,
    }

    if let Some(PatternNode::Metavar(metavar)) = significant.get(cursor).copied() {
        if capture.is_some() {
            return None;
        }
        capture = Some(metavar);
        cursor = cursor.saturating_add(1);
    }

    if let Some(PatternNode::Group(children)) = significant.get(cursor).copied() {
        if significant_children(children).is_empty() {
            return None;
        }
        for child_pattern in significant_children(children) {
            let _shape = lower_authority_node_shape(child_pattern)?;
        }
        child_sequence = Some(children);
        cursor = cursor.saturating_add(1);
    }

    if cursor != significant.len() {
        return None;
    }
    if expected_kind.is_none() && capture.is_none() {
        return None;
    }

    Some(StructuralAuthorityNodeShape {
        expected_kind,
        capture,
        child_sequence,
    })
}

fn significant_children(children: &[PatternNode]) -> Vec<&PatternNode> {
    children
        .iter()
        .filter(|child| match child {
            PatternNode::Literal(text) => !text.trim().is_empty(),
            PatternNode::Metavar(_)
            | PatternNode::Group(_)
            | PatternNode::HoleMany(_)
            | PatternNode::WildcardMany => true,
        })
        .collect()
}

fn collect_authority_candidates<'a>(
    pattern: &StructuralPattern,
    node: &'a ParseNode,
    ancestors: &mut Vec<&'a ParseNode>,
    source: &str,
    out: &mut Vec<StructuralAuthorityCandidate>,
) -> Result<(), StructuralError> {
    if let Some(binding) = match_pattern_at_node(pattern, node, ancestors.as_slice(), source)? {
        out.push(StructuralAuthorityCandidate::new(
            ByteSpan::new(node.byte_start, node.byte_end)?,
            binding,
        ));
    }
    ancestors.push(node);
    for child in &node.children {
        collect_authority_candidates(pattern, child, ancestors, source, out)?;
    }
    let _popped: Option<&ParseNode> = ancestors.pop();
    Ok(())
}

fn match_pattern_at_node(
    pattern: &StructuralPattern,
    node: &ParseNode,
    ancestors: &[&ParseNode],
    source: &str,
) -> Result<Option<StructuralBinding>, StructuralError> {
    let Some(binding) = match_node_pattern(pattern.root(), node, StructuralBinding::empty())?
    else {
        return Ok(None);
    };
    if !constraints_match(pattern, &binding, source)? {
        return Ok(None);
    }
    if !inside_patterns_match(pattern.inside(), ancestors, source)? {
        return Ok(None);
    }
    if outside_patterns_match(pattern.outside(), ancestors, source)? {
        return Ok(None);
    }
    Ok(Some(binding))
}

fn inside_patterns_match(
    patterns: &[StructuralPattern],
    ancestors: &[&ParseNode],
    source: &str,
) -> Result<bool, StructuralError> {
    for pattern in patterns {
        let mut matched = false;
        for (idx, ancestor) in ancestors.iter().enumerate() {
            let prefix = ancestors.get(..idx).unwrap_or(&[]);
            if match_pattern_at_node(pattern, ancestor, prefix, source)?.is_some() {
                matched = true;
                break;
            }
        }
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

fn outside_patterns_match(
    patterns: &[StructuralPattern],
    ancestors: &[&ParseNode],
    source: &str,
) -> Result<bool, StructuralError> {
    for pattern in patterns {
        for (idx, ancestor) in ancestors.iter().enumerate() {
            let prefix = ancestors.get(..idx).unwrap_or(&[]);
            if match_pattern_at_node(pattern, ancestor, prefix, source)?.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn constraints_match(
    pattern: &StructuralPattern,
    binding: &StructuralBinding,
    source: &str,
) -> Result<bool, StructuralError> {
    for constraint in pattern.constraints() {
        let Some(left_span) = binding.get(&constraint.left.name) else {
            return Ok(false);
        };
        let left_text = extract_source_text(source, left_span)?;
        let matched = match &constraint.right {
            ConstraintOperand::Hole(hole) => {
                let Some(right_span) = binding.get(&hole.name) else {
                    return Ok(false);
                };
                left_text == extract_source_text(source, right_span)?
            }
            ConstraintOperand::Phrase(text) | ConstraintOperand::RawString(text) => {
                left_text == text
            }
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

fn extract_source_text(source: &str, span: ByteSpan) -> Result<&str, StructuralError> {
    let start = usize::try_from(span.start()).map_err(|err| {
        StructuralError::new(
            StructuralErrorCode::StrParseFail,
            format!("byte span start does not fit usize: {err}"),
        )
    })?;
    let end = usize::try_from(span.end()).map_err(|err| {
        StructuralError::new(
            StructuralErrorCode::StrParseFail,
            format!("byte span end does not fit usize: {err}"),
        )
    })?;
    source.get(start..end).ok_or_else(|| {
        StructuralError::new(
            StructuralErrorCode::StrParseFail,
            format!("byte span {span} is not a valid UTF-8 slice boundary"),
        )
    })
}

fn match_node_pattern(
    pattern: &PatternNode,
    node: &ParseNode,
    mut binding: StructuralBinding,
) -> Result<Option<StructuralBinding>, StructuralError> {
    match pattern {
        PatternNode::Literal(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() || node.kind.as_ref() == trimmed {
                Ok(Some(binding))
            } else {
                Ok(None)
            }
        }
        PatternNode::Metavar(metavar) | PatternNode::HoleMany(metavar) => {
            let span = ByteSpan::new(node.byte_start, node.byte_end)?;
            bind_metavar(&mut binding, metavar, span)?;
            Ok(Some(binding))
        }
        PatternNode::WildcardMany => Ok(Some(binding)),
        PatternNode::Group(children) => match_group_pattern(children, node, binding),
    }
}

fn match_group_pattern(
    children: &[PatternNode],
    node: &ParseNode,
    mut binding: StructuralBinding,
) -> Result<Option<StructuralBinding>, StructuralError> {
    let significant = significant_children(children);
    if significant.is_empty() {
        return Ok(Some(binding));
    }
    if significant.len() == 1 {
        let Some(single) = significant.first().copied() else {
            return Ok(Some(binding));
        };
        return match_node_pattern(single, node, binding);
    }

    let mut cursor: usize = 0;
    if let Some(PatternNode::Literal(text)) = significant.get(cursor).copied() {
        let trimmed = text.trim();
        if trimmed.is_empty() || node.kind.as_ref() != trimmed {
            return Ok(None);
        }
        cursor = cursor.saturating_add(1);
    }
    if let Some(pattern) = significant.get(cursor).copied() {
        match pattern {
            PatternNode::Metavar(metavar) | PatternNode::HoleMany(metavar) => {
                bind_metavar(
                    &mut binding,
                    metavar,
                    ByteSpan::new(node.byte_start, node.byte_end)?,
                )?;
                cursor = cursor.saturating_add(1);
            }
            PatternNode::WildcardMany => {
                cursor = cursor.saturating_add(1);
            }
            PatternNode::Literal(_) | PatternNode::Group(_) => {}
        }
    }
    if cursor >= significant.len() {
        return Ok(Some(binding));
    }
    let Some(remaining) = significant.get(cursor..) else {
        return Ok(Some(binding));
    };
    if remaining.len() == 1
        && let Some(PatternNode::Group(sequence_children)) = remaining.first().copied()
    {
        let unwrapped = significant_children(sequence_children);
        return match_child_sequence(&unwrapped, &node.children, binding);
    }
    match_child_sequence(remaining, &node.children, binding)
}

fn match_child_sequence(
    pattern_children: &[&PatternNode],
    tree_children: &[ParseNode],
    binding: StructuralBinding,
) -> Result<Option<StructuralBinding>, StructuralError> {
    if pattern_children.is_empty() {
        return Ok(Some(binding));
    }
    for start in 0..=tree_children.len() {
        if let Some(candidate) =
            match_child_sequence_from(pattern_children, tree_children, start, binding.clone())?
        {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn match_child_sequence_from(
    pattern_children: &[&PatternNode],
    tree_children: &[ParseNode],
    tree_index: usize,
    binding: StructuralBinding,
) -> Result<Option<StructuralBinding>, StructuralError> {
    let Some((first, rest)) = pattern_children.split_first() else {
        return Ok(Some(binding));
    };
    match first {
        PatternNode::HoleMany(metavar) => {
            let available = tree_children.len().saturating_sub(tree_index);
            for consume in (0..=available).rev() {
                let mut candidate_binding = binding.clone();
                bind_metavar(
                    &mut candidate_binding,
                    metavar,
                    capture_sibling_span(tree_children, tree_index, consume)?,
                )?;
                if let Some(next) = match_child_sequence_from(
                    rest,
                    tree_children,
                    tree_index.saturating_add(consume),
                    candidate_binding,
                )? {
                    return Ok(Some(next));
                }
            }
            Ok(None)
        }
        PatternNode::WildcardMany => {
            let available = tree_children.len().saturating_sub(tree_index);
            for consume in (0..=available).rev() {
                if let Some(next) = match_child_sequence_from(
                    rest,
                    tree_children,
                    tree_index.saturating_add(consume),
                    binding.clone(),
                )? {
                    return Ok(Some(next));
                }
            }
            Ok(None)
        }
        pattern @ (PatternNode::Literal(_) | PatternNode::Metavar(_) | PatternNode::Group(_)) => {
            let Some(tree_child) = tree_children.get(tree_index) else {
                return Ok(None);
            };
            let Some(next_binding) = match_node_pattern(pattern, tree_child, binding)? else {
                return Ok(None);
            };
            match_child_sequence_from(
                rest,
                tree_children,
                tree_index.saturating_add(1),
                next_binding,
            )
        }
    }
}

fn capture_sibling_span(
    tree_children: &[ParseNode],
    start: usize,
    count: usize,
) -> Result<ByteSpan, StructuralError> {
    if count == 0 {
        let boundary = tree_children.get(start).map_or_else(
            || tree_children.last().map_or(0, |node| node.byte_end),
            |node| node.byte_start,
        );
        return ByteSpan::new(boundary, boundary);
    }
    let Some(first) = tree_children.get(start) else {
        return Err(StructuralError::new(
            StructuralErrorCode::StrParseFail,
            "variadic structural capture start index exceeded available children",
        ));
    };
    let end_index = start.saturating_add(count.saturating_sub(1));
    let Some(last) = tree_children.get(end_index) else {
        return Err(StructuralError::new(
            StructuralErrorCode::StrParseFail,
            "variadic structural capture end index exceeded available children",
        ));
    };
    ByteSpan::new(first.byte_start, last.byte_end)
}

fn bind_metavar(
    binding: &mut StructuralBinding,
    metavar: &MetaVar,
    span: ByteSpan,
) -> Result<(), StructuralError> {
    match binding.get(metavar) {
        Some(existing) if existing != span => Err(StructuralError::new(
            StructuralErrorCode::StrParseFail,
            format!(
                "metavariable `${}` matched conflicting spans {existing} and {span}",
                metavar.as_str(),
            ),
        )),
        Some(_existing) => Ok(()),
        None => {
            let _prior = binding.insert(metavar.clone(), span);
            Ok(())
        }
    }
}
