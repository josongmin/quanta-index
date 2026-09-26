//! Lowering predicates into the boolean scope of a query.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::documents::collapse_exprs;
use crate::predicate_registry::{
    ContentPredicateConstraint, PREDICATE_OWNER, PredicateKind, kind_of, unimplemented_predicate,
};
use crate::{PreparedPredicatePlan, RepoScopeConstraint, TantivySearcher};
use quanta_index_contract::{LqExpr, LqLeaf, LqPredicateArg, LqQuery};
use quanta_index_core::{CoreError, RequestBudgetV1};
use std::collections::BTreeSet;

impl TantivySearcher {
    pub(crate) fn lower_predicate_for_boolean_scope(
        &self,
        name: &str,
        args: &[LqPredicateArg],
        budget: &RequestBudgetV1,
    ) -> Result<LqExpr, CoreError> {
        if let Some(predicate) = quanta_index_core::LexicalPredicateV1::from_canonical_name(name)
            && predicate.exact_symbol_name_argument(args)?.is_some()
        {
            return Ok(LqExpr::Leaf(LqLeaf::Predicate {
                name: name.to_owned(),
                args: args.to_vec(),
            }));
        }
        let Some((canonical_name, canonical_args)) =
            self.canonicalize_predicate_call(name, args)?
        else {
            return Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            )));
        };
        match kind_of(&canonical_name) {
            Some(PredicateKind::RepoFileGate) => {
                let _constraint =
                    self.repo_has_file_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoContentGate) => {
                let _leaf = self.repo_content_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoCommitRecencyGate) => {
                let _timeref =
                    self.repo_commit_after_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoMetaGate) => {
                let _arg = self.repo_meta_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoTopicGate) => {
                let _arg = self.repo_topic_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoDescriptionGate) => {
                let _arg = self.repo_description_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::FileOwnerGate) => {
                let _arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::FileContributorGate) => {
                let _arg = self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::ContentLeaf) => {
                let constraint =
                    self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                drop(self.content_predicate_match_set(&constraint, budget)?);
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            None => Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            ))),
        }
    }

    pub(crate) fn lower_predicates_for_boolean_scope(
        &self,
        expr: &LqExpr,
        budget: &RequestBudgetV1,
    ) -> Result<LqExpr, CoreError> {
        match expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                self.lower_predicate_for_boolean_scope(name, args, budget)
            }
            LqExpr::Empty | LqExpr::Leaf(_) => Ok(expr.clone()),
            LqExpr::All(parts) => {
                let mut out: Vec<LqExpr> = Vec::with_capacity(parts.len());
                for part in parts {
                    out.push(self.lower_predicates_for_boolean_scope(part, budget)?);
                }
                Ok(collapse_exprs(out, true))
            }
            LqExpr::Any(parts) => {
                let mut out: Vec<LqExpr> = Vec::with_capacity(parts.len());
                for part in parts {
                    out.push(self.lower_predicates_for_boolean_scope(part, budget)?);
                }
                Ok(collapse_exprs(out, false))
            }
            LqExpr::Not(inner) => Ok(LqExpr::Not(Box::new(
                self.lower_predicates_for_boolean_scope(inner, budget)?,
            ))),
        }
    }

    pub(crate) fn extract_predicate_plan(
        &self,
        expr: &LqExpr,
        budget: &RequestBudgetV1,
    ) -> Result<
        (
            LqExpr,
            Vec<RepoScopeConstraint>,
            Vec<ContentPredicateConstraint>,
        ),
        CoreError,
    > {
        match expr {
            LqExpr::Empty => Ok((LqExpr::Empty, Vec::new(), Vec::new())),
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                if let Some(predicate) =
                    quanta_index_core::LexicalPredicateV1::from_canonical_name(name)
                    && predicate.exact_symbol_name_argument(args)?.is_some()
                {
                    return Ok((expr.clone(), Vec::new(), Vec::new()));
                }
                let Some((canonical_name, canonical_args)) =
                    self.canonicalize_predicate_call(name, args)?
                else {
                    return Err(unimplemented_predicate(format!(
                        "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                    )));
                };
                match kind_of(&canonical_name) {
                    Some(PredicateKind::RepoFileGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::File(self.repo_has_file_constraint(
                            &canonical_name,
                            &canonical_args,
                        )?)],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoContentGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Content(
                            self.repo_content_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoCommitRecencyGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::CommitAfter(
                            self.repo_commit_after_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoMetaGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Meta(
                            self.repo_meta_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoTopicGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Topic(
                            self.repo_topic_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoDescriptionGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Description(
                            self.repo_description_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::FileOwnerGate) => {
                        let _arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                        Ok((
                            LqExpr::Leaf(LqLeaf::Predicate {
                                name: canonical_name,
                                args: canonical_args,
                            }),
                            Vec::new(),
                            Vec::new(),
                        ))
                    }
                    Some(PredicateKind::FileContributorGate) => {
                        let _arg =
                            self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                        Ok((
                            LqExpr::Leaf(LqLeaf::Predicate {
                                name: canonical_name,
                                args: canonical_args,
                            }),
                            Vec::new(),
                            Vec::new(),
                        ))
                    }
                    Some(PredicateKind::ContentLeaf) => Ok((
                        LqExpr::Empty,
                        Vec::new(),
                        vec![self.content_predicate_constraint(&canonical_name, &canonical_args)?],
                    )),
                    None => Err(unimplemented_predicate(format!(
                        "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                    ))),
                }
            }
            LqExpr::Leaf(_) => Ok((expr.clone(), Vec::new(), Vec::new())),
            LqExpr::All(parts) => {
                let mut exprs: Vec<LqExpr> = Vec::new();
                let mut repo_predicates: Vec<RepoScopeConstraint> = Vec::new();
                let mut file_predicates: Vec<ContentPredicateConstraint> = Vec::new();
                for part in parts {
                    let (lowered, repo_parts, file_parts) =
                        self.extract_predicate_plan(part, budget)?;
                    if !matches!(lowered, LqExpr::Empty) {
                        exprs.push(lowered);
                    }
                    repo_predicates.extend(repo_parts);
                    file_predicates.extend(file_parts);
                }
                Ok((
                    collapse_exprs(exprs, true),
                    repo_predicates,
                    file_predicates,
                ))
            }
            LqExpr::Any(_) | LqExpr::Not(_) => Ok((
                self.lower_predicates_for_boolean_scope(expr, budget)?,
                Vec::new(),
                Vec::new(),
            )),
        }
    }

    pub(crate) fn prepare_predicate_plan(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<PreparedPredicatePlan, CoreError> {
        if let LqExpr::Leaf(LqLeaf::Predicate { name, args }) = &query.expr {
            if let Some(predicate) =
                quanta_index_core::LexicalPredicateV1::from_canonical_name(name)
                && predicate.exact_symbol_name_argument(args)?.is_some()
            {
                return Ok(PreparedPredicatePlan {
                    expr: query.expr.clone(),
                    allowed_paths: None,
                    allowed_repo_ids: None,
                    allowed_candidate_ids: None,
                    force_empty: false,
                });
            }
            let Some((canonical_name, canonical_args)) =
                self.canonicalize_predicate_call(name, args)?
            else {
                return Err(unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                )));
            };
            if matches!(kind_of(&canonical_name), Some(PredicateKind::ContentLeaf)) {
                let constraint =
                    self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                let allowed_paths =
                    self.collect_matching_paths_for_content_scope(&constraint, budget)?;
                if allowed_paths.as_ref().is_some_and(BTreeSet::is_empty) {
                    return Ok(PreparedPredicatePlan {
                        expr: LqExpr::Empty,
                        allowed_paths: None,
                        allowed_repo_ids: None,
                        allowed_candidate_ids: None,
                        force_empty: true,
                    });
                }
                let lowered = self.predicate_content_leaf_from_constraint(&constraint);
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Leaf(lowered),
                    allowed_paths,
                    allowed_repo_ids: None,
                    allowed_candidate_ids: None,
                    force_empty: false,
                });
            }
        }
        let (expr, repo_constraints, file_predicates) =
            self.extract_predicate_plan(&query.expr, budget)?;
        let mut allowed_repo_ids: Option<BTreeSet<String>> = None;
        for constraint in &repo_constraints {
            let repo_ids = match constraint {
                RepoScopeConstraint::File(file) => {
                    self.collect_repo_ids_for_repo_has_file(file, &query.options, budget)?
                }
                RepoScopeConstraint::Content(leaf) => {
                    self.collect_repo_ids_for_repo_has_content(leaf, &query.options, budget)?
                }
                RepoScopeConstraint::CommitAfter(timeref) => {
                    self.collect_repo_ids_for_repo_has_commit_after(timeref)?
                }
                RepoScopeConstraint::Meta(arg) => self.collect_repo_ids_for_repo_has_meta(arg)?,
                RepoScopeConstraint::Topic(arg) => self.collect_repo_ids_for_repo_has_topic(arg)?,
                RepoScopeConstraint::Description(arg) => {
                    self.collect_repo_ids_for_repo_has_description(arg)?
                }
            };
            if repo_ids.is_empty() {
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Empty,
                    allowed_paths: None,
                    allowed_repo_ids: None,
                    allowed_candidate_ids: None,
                    force_empty: true,
                });
            }
            allowed_repo_ids = Some(match allowed_repo_ids.take() {
                Some(existing) => existing.intersection(&repo_ids).cloned().collect(),
                None => repo_ids,
            });
        }
        if allowed_repo_ids.as_ref().is_some_and(BTreeSet::is_empty) {
            return Ok(PreparedPredicatePlan {
                expr: LqExpr::Empty,
                allowed_paths: None,
                allowed_repo_ids: None,
                allowed_candidate_ids: None,
                force_empty: true,
            });
        }
        let mut allowed_paths: Option<BTreeSet<String>> = None;
        for predicate in &file_predicates {
            let paths = self.allowed_paths_for_content_predicate(predicate, budget)?;
            if paths.is_empty() {
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Empty,
                    allowed_paths: None,
                    allowed_repo_ids: None,
                    allowed_candidate_ids: None,
                    force_empty: true,
                });
            }
            allowed_paths = Some(match allowed_paths.take() {
                Some(existing) => existing.intersection(&paths).cloned().collect(),
                None => paths,
            });
        }
        if allowed_paths.as_ref().is_some_and(BTreeSet::is_empty) {
            return Ok(PreparedPredicatePlan {
                expr: LqExpr::Empty,
                allowed_paths: None,
                allowed_repo_ids: None,
                allowed_candidate_ids: None,
                force_empty: true,
            });
        }
        Ok(PreparedPredicatePlan {
            expr,
            allowed_paths,
            allowed_repo_ids,
            allowed_candidate_ids: None,
            force_empty: false,
        })
    }
}
