//! Structural pattern IR for authoritative `match { ... }` execution.
//!
//! [`StructuralPattern`] is the typed result of lowering a contract-layer
//! [`quanta_index_contract::LqStructuralBlock`] into engine IR.
//! [`PatternNode`] is the node-level IR: literals, metavariables, holes,
//! wildcards, and nested groups.
//!
//! Caps:
//!
//! - [`crate::types::MAX_STRUCTURAL_NODES`] (256) — total node count.
//! - [`crate::types::MAX_DEPTH`] (16) — nesting depth.
//! - [`crate::types::MAX_METAVARS_PER_PATTERN`] (32) — distinct metavar
//!   names.
//!
//! Any breach surfaces a typed [`crate::errors::StructuralError`].
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;
use std::collections::BTreeSet;

use crate::errors::{LimitDimension, StructuralError, StructuralErrorCode};
use crate::types::{LangId, MAX_DEPTH, MAX_METAVARS_PER_PATTERN, MAX_STRUCTURAL_NODES, MetaVar};

/// Single pattern-IR node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PatternNode {
    /// Verbatim text segment.
    Literal(Box<str>),
    /// Metavariable capture (`$name` / `:[name]`).
    Metavar(MetaVar),
    /// Variadic contiguous sibling capture (`$...name` / `:[...name]`).
    HoleMany(MetaVar),
    /// Anonymous variadic contiguous sibling wildcard (`...`).
    WildcardMany,
    /// Brace-delimited group; sequence of child nodes in source order.
    Group(Vec<PatternNode>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoleMultiplicity {
    One,
    Many,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HoleRef {
    pub name: MetaVar,
    pub multiplicity: HoleMultiplicity,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConstraintOperand {
    Hole(HoleRef),
    Phrase(String),
    RawString(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructuralConstraint {
    pub left: HoleRef,
    pub right: ConstraintOperand,
}

impl PatternNode {
    /// Count this node plus every descendant.
    pub(crate) fn count_nodes(&self, acc: &mut u32) -> Result<(), StructuralError> {
        *acc = acc.checked_add(1).ok_or_else(|| {
            StructuralError::plan_limit_exceeded(
                LimitDimension::NodeCount,
                "node count overflowed u32",
            )
        })?;
        if *acc > MAX_STRUCTURAL_NODES {
            return Err(StructuralError::plan_limit_exceeded(
                LimitDimension::NodeCount,
                format!("structural pattern exceeds MAX_STRUCTURAL_NODES={MAX_STRUCTURAL_NODES}"),
            ));
        }
        match self {
            Self::Literal(_) | Self::Metavar(_) | Self::HoleMany(_) | Self::WildcardMany => Ok(()),
            Self::Group(children) => {
                for c in children {
                    c.count_nodes(acc)?;
                }
                Ok(())
            }
        }
    }

    /// Walk depth.
    pub(crate) fn check_depth(&self, current: u32) -> Result<(), StructuralError> {
        if current > MAX_DEPTH {
            return Err(StructuralError::plan_limit_exceeded(
                LimitDimension::Depth,
                format!("structural pattern depth exceeds MAX_DEPTH={MAX_DEPTH}"),
            ));
        }
        match self {
            Self::Literal(_) | Self::Metavar(_) | Self::HoleMany(_) | Self::WildcardMany => Ok(()),
            Self::Group(children) => {
                let next = current.checked_add(1).ok_or_else(|| {
                    StructuralError::plan_limit_exceeded(
                        LimitDimension::Depth,
                        "depth counter overflowed u32",
                    )
                })?;
                for c in children {
                    c.check_depth(next)?;
                }
                Ok(())
            }
        }
    }

    /// Collect every metavariable name reachable from this node.
    pub(crate) fn collect_metavars(&self, into: &mut BTreeSet<MetaVar>) {
        match self {
            Self::Literal(_) => {}
            Self::Metavar(m) => {
                let _inserted: bool = into.insert(m.clone());
            }
            Self::HoleMany(m) => {
                let _inserted: bool = into.insert(m.clone());
            }
            Self::WildcardMany => {}
            Self::Group(children) => {
                for c in children {
                    c.collect_metavars(into);
                }
            }
        }
    }
}

impl serde::Serialize for PatternNode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Literal(s) => {
                m.serialize_entry("kind", "LITERAL")?;
                m.serialize_entry("value", s.as_ref())?;
            }
            Self::Metavar(v) => {
                m.serialize_entry("kind", "METAVAR")?;
                m.serialize_entry("value", v)?;
            }
            Self::HoleMany(v) => {
                m.serialize_entry("kind", "HOLE_MANY")?;
                m.serialize_entry("value", v)?;
            }
            Self::WildcardMany => {
                m.serialize_entry("kind", "WILDCARD_MANY")?;
                m.serialize_entry("value", &())?;
            }
            Self::Group(children) => {
                m.serialize_entry("kind", "GROUP")?;
                m.serialize_entry("value", children)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for PatternNode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        enum Held {
            None,
            Literal(String),
            Metavar(MetaVar),
            HoleMany(MetaVar),
            WildcardMany,
            Group(Vec<PatternNode>),
        }

        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = PatternNode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("PatternNode map (kind, value)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<PatternNode, M::Error> {
                let mut kind: Option<String> = None;
                let mut held: Held = Held::None;
                let mut value_seen = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => {
                            if kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        "value" => {
                            if value_seen {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            value_seen = true;
                            let Some(k) = kind.as_deref() else {
                                return Err(serde::de::Error::custom(
                                    "PatternNode 'kind' must precede 'value'",
                                ));
                            };
                            held = match k {
                                "LITERAL" => Held::Literal(map.next_value()?),
                                "METAVAR" => Held::Metavar(map.next_value()?),
                                "HOLE_MANY" => Held::HoleMany(map.next_value()?),
                                "WILDCARD_MANY" => {
                                    let _unit: () = map.next_value()?;
                                    Held::WildcardMany
                                }
                                "GROUP" => Held::Group(map.next_value()?),
                                other => {
                                    return Err(serde::de::Error::unknown_variant(
                                        other,
                                        &[
                                            "LITERAL",
                                            "METAVAR",
                                            "HOLE_MANY",
                                            "WILDCARD_MANY",
                                            "GROUP",
                                        ],
                                    ));
                                }
                            };
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &["kind", "value"]));
                        }
                    }
                }
                let _kind = kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                match held {
                    Held::None => Err(serde::de::Error::missing_field("value")),
                    Held::Literal(s) => Ok(PatternNode::Literal(s.into_boxed_str())),
                    Held::Metavar(m) => Ok(PatternNode::Metavar(m)),
                    Held::HoleMany(m) => Ok(PatternNode::HoleMany(m)),
                    Held::WildcardMany => Ok(PatternNode::WildcardMany),
                    Held::Group(children) => Ok(PatternNode::Group(children)),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for HoleMultiplicity {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(match self {
            Self::One => "ONE",
            Self::Many => "MANY",
        })
    }
}

impl<'de> serde::Deserialize<'de> for HoleMultiplicity {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = HoleMultiplicity;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("HoleMultiplicity string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<HoleMultiplicity, E> {
                match v {
                    "ONE" => Ok(HoleMultiplicity::One),
                    "MANY" => Ok(HoleMultiplicity::Many),
                    other => Err(E::unknown_variant(other, &["ONE", "MANY"])),
                }
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for HoleRef {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("multiplicity", &self.multiplicity)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for HoleRef {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HoleRef;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("HoleRef map")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<HoleRef, M::Error> {
                let mut name: Option<MetaVar> = None;
                let mut multiplicity: Option<HoleMultiplicity> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => name = Some(map.next_value()?),
                        "multiplicity" => multiplicity = Some(map.next_value()?),
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["name", "multiplicity"],
                            ));
                        }
                    }
                }
                Ok(HoleRef {
                    name: name.ok_or_else(|| serde::de::Error::missing_field("name"))?,
                    multiplicity: multiplicity
                        .ok_or_else(|| serde::de::Error::missing_field("multiplicity"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for ConstraintOperand {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        match self {
            Self::Hole(hole) => {
                m.serialize_entry("kind", "HOLE")?;
                m.serialize_entry("value", hole)?;
            }
            Self::Phrase(text) => {
                m.serialize_entry("kind", "PHRASE")?;
                m.serialize_entry("value", text)?;
            }
            Self::RawString(text) => {
                m.serialize_entry("kind", "RAW_STRING")?;
                m.serialize_entry("value", text)?;
            }
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for ConstraintOperand {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        enum Held {
            None,
            Hole(HoleRef),
            Phrase(String),
            RawString(String),
        }

        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = ConstraintOperand;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ConstraintOperand map")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<ConstraintOperand, M::Error> {
                let mut kind: Option<String> = None;
                let mut held = Held::None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => kind = Some(map.next_value()?),
                        "value" => {
                            let Some(kind) = kind.as_deref() else {
                                return Err(serde::de::Error::custom(
                                    "ConstraintOperand 'kind' must precede 'value'",
                                ));
                            };
                            held = match kind {
                                "HOLE" => Held::Hole(map.next_value()?),
                                "PHRASE" => Held::Phrase(map.next_value()?),
                                "RAW_STRING" => Held::RawString(map.next_value()?),
                                other => {
                                    return Err(serde::de::Error::unknown_variant(
                                        other,
                                        &["HOLE", "PHRASE", "RAW_STRING"],
                                    ));
                                }
                            };
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &["kind", "value"]));
                        }
                    }
                }
                match held {
                    Held::None => Err(serde::de::Error::missing_field("value")),
                    Held::Hole(v) => Ok(ConstraintOperand::Hole(v)),
                    Held::Phrase(v) => Ok(ConstraintOperand::Phrase(v)),
                    Held::RawString(v) => Ok(ConstraintOperand::RawString(v)),
                }
            }
        }
        de.deserialize_map(V)
    }
}

impl serde::Serialize for StructuralConstraint {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("left", &self.left)?;
        m.serialize_entry("right", &self.right)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for StructuralConstraint {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = StructuralConstraint;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("StructuralConstraint map")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<StructuralConstraint, M::Error> {
                let mut left: Option<HoleRef> = None;
                let mut right: Option<ConstraintOperand> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "left" => left = Some(map.next_value()?),
                        "right" => right = Some(map.next_value()?),
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &["left", "right"]));
                        }
                    }
                }
                Ok(StructuralConstraint {
                    left: left.ok_or_else(|| serde::de::Error::missing_field("left"))?,
                    right: right.ok_or_else(|| serde::de::Error::missing_field("right"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// Parsed structural pattern, anchored to a single [`LangId`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructuralPattern {
    lang: LangId,
    root: PatternNode,
    constraints: Vec<StructuralConstraint>,
    inside: Vec<StructuralPattern>,
    outside: Vec<StructuralPattern>,
    metavars: BTreeSet<MetaVar>,
}

impl StructuralPattern {
    /// Construct directly without parsing. Useful for fixture / test code
    /// that wants to bypass the surface grammar. Enforces every cap.
    pub fn from_root(lang: LangId, root: PatternNode) -> Result<Self, StructuralError> {
        Self::from_parts(lang, root, Vec::new(), Vec::new(), Vec::new())
    }

    pub fn from_parts(
        lang: LangId,
        root: PatternNode,
        constraints: Vec<StructuralConstraint>,
        inside: Vec<StructuralPattern>,
        outside: Vec<StructuralPattern>,
    ) -> Result<Self, StructuralError> {
        let mut node_acc: u32 = 0;
        root.count_nodes(&mut node_acc)?;
        root.check_depth(0)?;
        let mut metavars: BTreeSet<MetaVar> = BTreeSet::new();
        root.collect_metavars(&mut metavars);
        validate_constraint_refs(&metavars, &constraints)?;
        for nested in inside.iter().chain(outside.iter()) {
            for mv in nested.metavars() {
                let _inserted: bool = metavars.insert(mv.clone());
            }
        }
        let observed_metavars: u32 = check_metavar_count(metavars.len())?;
        if observed_metavars > MAX_METAVARS_PER_PATTERN {
            return Err(StructuralError::plan_limit_exceeded(
                LimitDimension::MetavarCount,
                format!(
                    "structural pattern exceeds MAX_METAVARS_PER_PATTERN={MAX_METAVARS_PER_PATTERN} (observed={observed_metavars})"
                ),
            ));
        }
        Ok(Self {
            lang,
            root,
            constraints,
            inside,
            outside,
            metavars,
        })
    }

    /// Borrow the anchor language.
    #[must_use]
    pub const fn lang(&self) -> LangId {
        self.lang
    }

    /// Borrow the pattern's root IR node.
    #[must_use]
    pub const fn root(&self) -> &PatternNode {
        &self.root
    }

    #[must_use]
    pub fn constraints(&self) -> &[StructuralConstraint] {
        &self.constraints
    }

    #[must_use]
    pub fn inside(&self) -> &[StructuralPattern] {
        &self.inside
    }

    #[must_use]
    pub fn outside(&self) -> &[StructuralPattern] {
        &self.outside
    }

    /// Borrow the distinct metavariable set captured by this pattern.
    #[must_use]
    pub const fn metavars(&self) -> &BTreeSet<MetaVar> {
        &self.metavars
    }

    /// `true` if the pattern captures at least one metavariable.
    #[must_use]
    pub fn has_metavars(&self) -> bool {
        !self.metavars.is_empty()
    }
}

fn validate_constraint_refs(
    bound_metavars: &BTreeSet<MetaVar>,
    constraints: &[StructuralConstraint],
) -> Result<(), StructuralError> {
    for constraint in constraints {
        if !bound_metavars.contains(&constraint.left.name) {
            return Err(StructuralError::invalid_metavar(
                constraint.left.name.as_str(),
            ));
        }
        if let ConstraintOperand::Hole(hole) = &constraint.right
            && !bound_metavars.contains(&hole.name)
        {
            return Err(StructuralError::invalid_metavar(hole.name.as_str()));
        }
    }
    Ok(())
}

impl serde::Serialize for StructuralPattern {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(6))?;
        m.serialize_entry("lang", &self.lang)?;
        m.serialize_entry("root", &self.root)?;
        m.serialize_entry("constraints", &self.constraints)?;
        m.serialize_entry("inside", &self.inside)?;
        m.serialize_entry("outside", &self.outside)?;
        // Metavars are derived from root but emitted explicitly so the
        // wire shape is self-describing and matches the §4.9 carrier.
        let mv: Vec<&MetaVar> = self.metavars.iter().collect();
        m.serialize_entry("metavars", &mv)?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for StructuralPattern {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = StructuralPattern;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "StructuralPattern map (lang, root, constraints, inside, outside, metavars)",
                )
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<StructuralPattern, M::Error> {
                let mut lang: Option<LangId> = None;
                let mut root: Option<PatternNode> = None;
                let mut constraints: Option<Vec<StructuralConstraint>> = None;
                let mut inside: Option<Vec<StructuralPattern>> = None;
                let mut outside: Option<Vec<StructuralPattern>> = None;
                let mut metavars_in: Option<Vec<MetaVar>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "lang" => {
                            if lang.is_some() {
                                return Err(serde::de::Error::duplicate_field("lang"));
                            }
                            lang = Some(map.next_value()?);
                        }
                        "root" => {
                            if root.is_some() {
                                return Err(serde::de::Error::duplicate_field("root"));
                            }
                            root = Some(map.next_value()?);
                        }
                        "constraints" => {
                            if constraints.is_some() {
                                return Err(serde::de::Error::duplicate_field("constraints"));
                            }
                            constraints = Some(map.next_value()?);
                        }
                        "inside" => {
                            if inside.is_some() {
                                return Err(serde::de::Error::duplicate_field("inside"));
                            }
                            inside = Some(map.next_value()?);
                        }
                        "outside" => {
                            if outside.is_some() {
                                return Err(serde::de::Error::duplicate_field("outside"));
                            }
                            outside = Some(map.next_value()?);
                        }
                        "metavars" => {
                            if metavars_in.is_some() {
                                return Err(serde::de::Error::duplicate_field("metavars"));
                            }
                            metavars_in = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &[
                                    "lang",
                                    "root",
                                    "constraints",
                                    "inside",
                                    "outside",
                                    "metavars",
                                ],
                            ));
                        }
                    }
                }
                let lang = lang.ok_or_else(|| serde::de::Error::missing_field("lang"))?;
                let root = root.ok_or_else(|| serde::de::Error::missing_field("root"))?;
                let constraints =
                    constraints.ok_or_else(|| serde::de::Error::missing_field("constraints"))?;
                let inside = inside.ok_or_else(|| serde::de::Error::missing_field("inside"))?;
                let outside = outside.ok_or_else(|| serde::de::Error::missing_field("outside"))?;
                let declared =
                    metavars_in.ok_or_else(|| serde::de::Error::missing_field("metavars"))?;
                let pattern =
                    StructuralPattern::from_parts(lang, root, constraints, inside, outside)
                        .map_err(|e| serde::de::Error::custom(format!("{e}")))?;
                let declared_set: BTreeSet<MetaVar> = declared.into_iter().collect();
                if declared_set != *pattern.metavars() {
                    return Err(serde::de::Error::custom(
                        "StructuralPattern metavars mismatch between declared list and derived structure",
                    ));
                }
                Ok(pattern)
            }
        }
        de.deserialize_map(V)
    }
}

/// Saturating cast of a `usize` metavar-count to `u32`.
///
/// Returns `PLAN_LIMIT_EXCEEDED { METAVAR_COUNT }` if the count cannot
/// fit in `u32` (which already implies it exceeds the cap).
#[expect(
    clippy::option_if_let_else,
    reason = "the clippy-suggested `.map_or_else(...)` form then trips `unnecessary_result_map_or_else` because the success arm is the identity"
)]
fn check_metavar_count(len: usize) -> Result<u32, StructuralError> {
    if let Ok(v) = u32::try_from(len) {
        Ok(v)
    } else {
        Err(StructuralError::plan_limit_exceeded(
            LimitDimension::MetavarCount,
            format!("metavar count {len} exceeds u32 range (cap is {MAX_METAVARS_PER_PATTERN})"),
        ))
    }
}

// Tag this so callers can pattern-match against a sentinel error code
// shape rather than re-parsing the detail string.
impl StructuralError {
    /// `true` if `self.code == StrInvalidMetavar`.
    #[must_use]
    pub fn is_invalid_metavar(&self) -> bool {
        matches!(self.code, StructuralErrorCode::StrInvalidMetavar)
    }
}

#[cfg(test)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "tests pin exact pattern shapes and surface unexpected failures via assert!(false, …)"
)]
mod tests {
    use quanta_index_contract::{
        LqMetaVar, LqStructuralBlock, LqStructuralExpr, LqStructuralHoleMultiplicity,
        LqStructuralNode,
    };

    use super::{PatternNode, StructuralPattern};
    use crate::errors::{LimitDimension, StructuralErrorCode};
    use crate::matcher::compile_authoritative_pattern;
    use crate::types::{LangId, MAX_DEPTH, MAX_METAVARS_PER_PATTERN, MetaVar};

    fn literal(text: &str) -> LqStructuralNode {
        LqStructuralNode::Literal(text.to_string().into_boxed_str())
    }

    fn metavar(name: &str) -> LqStructuralNode {
        LqStructuralNode::MetaVar(LqMetaVar::new(name.to_string()))
    }

    fn group(children: Vec<LqStructuralNode>) -> LqStructuralNode {
        LqStructuralNode::Group(children)
    }

    fn hole_many(name: &str) -> LqStructuralNode {
        LqStructuralNode::Hole {
            name: Some(LqMetaVar::new(name.to_string())),
            multiplicity: LqStructuralHoleMultiplicity::Many,
        }
    }

    fn compile_block(
        nodes: Vec<LqStructuralNode>,
        lang: &str,
    ) -> Result<StructuralPattern, crate::errors::StructuralError> {
        compile_authoritative_pattern(
            &LqStructuralBlock {
                lang: None,
                nodes: nodes.clone(),
                exprs: vec![LqStructuralExpr::Pattern(nodes)],
            },
            lang,
        )
    }

    #[test]
    fn compile_pure_literal() {
        let Ok(p) = compile_block(vec![literal("hello")], "rust") else {
            assert!(false, "must compile literal");
            return;
        };
        match p.root() {
            PatternNode::Literal(s) => assert_eq!(s.as_ref(), "hello"),
            other => assert!(false, "expected literal root, got {other:?}"),
        }
        assert!(p.metavars().is_empty());
    }

    #[test]
    fn compile_single_metavar() {
        let Ok(p) = compile_block(vec![metavar("X")], "rust") else {
            assert!(false, "must compile metavar");
            return;
        };
        match p.root() {
            PatternNode::Metavar(m) => assert_eq!(m.as_str(), "X"),
            other => assert!(false, "expected metavar root, got {other:?}"),
        }
        assert_eq!(p.metavars().len(), 1);
    }

    #[test]
    fn compile_named_hole_one_lowers_to_metavar() {
        let Ok(p) = compile_block(
            vec![LqStructuralNode::Hole {
                name: Some(LqMetaVar::new("X".to_string())),
                multiplicity: LqStructuralHoleMultiplicity::One,
            }],
            "rust",
        ) else {
            assert!(false, "must compile named hole");
            return;
        };
        match p.root() {
            PatternNode::Metavar(m) => assert_eq!(m.as_str(), "X"),
            other => assert!(false, "expected lowered metavar, got {other:?}"),
        }
    }

    #[test]
    fn compile_nested_group() {
        let Ok(p) = compile_block(
            vec![
                literal("fn "),
                metavar("name"),
                literal("(arg) "),
                group(vec![literal("body")]),
            ],
            "rust",
        ) else {
            assert!(false, "must compile nested group");
            return;
        };
        let PatternNode::Group(children) = p.root() else {
            assert!(false, "root must be group");
            return;
        };
        // children: literal "fn ", metavar name, literal "(arg) ", group { ... }
        assert_eq!(children.len(), 4, "got: {children:?}");
        let Some(fourth) = children.get(3) else {
            assert!(false, "expected 4 children");
            return;
        };
        match fourth {
            PatternNode::Group(_) => {}
            other => assert!(false, "expected group, got {other:?}"),
        }
    }

    #[test]
    fn compile_metavar_dedup_in_set() {
        let Ok(p) = compile_block(vec![metavar("X"), literal(" and "), metavar("X")], "rust")
        else {
            assert!(false, "must compile dedup case");
            return;
        };
        assert_eq!(p.metavars().len(), 1);
    }

    #[test]
    fn depth_cap_enforced() {
        let mut node = literal("leaf");
        for _ in 0..MAX_DEPTH.saturating_add(2) {
            node = group(vec![node]);
        }
        match compile_block(vec![node], "rust") {
            Ok(_) => assert!(false, "depth cap must trip"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::Depth));
            }
        }
    }

    #[test]
    fn node_cap_enforced() {
        match compile_block(vec![group(Vec::new()); 300], "rust") {
            Ok(_) => assert!(false, "node cap must trip"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::NodeCount));
            }
        }
    }

    #[test]
    fn metavar_cap_enforced() {
        let mut nodes: Vec<LqStructuralNode> = Vec::new();
        let cap_plus_one = MAX_METAVARS_PER_PATTERN.saturating_add(1);
        for i in 0..cap_plus_one {
            nodes.push(metavar(&format!("x{i}")));
        }
        match compile_block(nodes, "rust") {
            Ok(_) => assert!(false, "metavar cap must trip"),
            Err(e) => {
                assert_eq!(e.code, StructuralErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::MetavarCount));
            }
        }
    }

    #[test]
    fn variadic_hole_many_is_preserved_in_pattern_ir() {
        let Ok(p) = compile_block(vec![hole_many("ARGS")], "rust") else {
            assert!(false, "must compile variadic hole");
            return;
        };
        match p.root() {
            PatternNode::HoleMany(m) => assert_eq!(m.as_str(), "ARGS"),
            other => assert!(false, "expected variadic hole root, got {other:?}"),
        }
        assert_eq!(p.metavars().len(), 1);
    }

    #[test]
    fn from_root_constructs_pattern() {
        let Ok(m) = MetaVar::new("Y") else {
            assert!(false, "metavar");
            return;
        };
        let root = PatternNode::Group(vec![PatternNode::Metavar(m.clone())]);
        let Ok(p) = StructuralPattern::from_root(LangId::Python, root) else {
            assert!(false, "from_root must succeed");
            return;
        };
        assert_eq!(p.lang(), LangId::Python);
        assert!(p.has_metavars());
        assert!(p.metavars().contains(&m));
    }

    #[test]
    fn pattern_serde_roundtrip_via_ciborium() {
        let Ok(p) = compile_block(
            vec![
                literal("fn "),
                metavar("name"),
                literal("() "),
                group(vec![literal("body")]),
            ],
            "rust",
        ) else {
            assert!(false, "must compile");
            return;
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&p, &mut buf) {
            assert!(false, "ser: {e}");
        }
        let got: Result<StructuralPattern, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, p),
            Err(e) => assert!(false, "de: {e}"),
        }
    }
}
