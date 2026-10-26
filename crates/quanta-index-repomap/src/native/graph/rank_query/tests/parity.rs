//! Native scan/projection parity against an independent full-sort budget walk.
//! The oracle shares only the public text normalizer, never the rank selector.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::{Arc, Mutex, atomic::Ordering};

use quanta_index_contract::{
    ChunkId, CompiledRepoMapProjectionEntryV1, RepoMapDocType, RepoMapFocusSubjectDto,
    RepoMapModuleId, RepoMapNodeRef, RepoRelativePath, SymbolId,
    index::{Digest, MapRoot, ObjectRef},
    native_protocol::{NativeRepoMapHit, NativeRepoMapQuery},
};
use quanta_index_core::index::{IndexError, MemoryScope};
use quanta_index_lq_text_normalizer::{CaseMode, fold, tokenize};

use super::{
    Maps, Objects, PageMemoryCount, PageMemoryScope, budget, file, node_key, owner, query_rank,
    reference, validate_request,
};
use crate::native::graph::{
    GraphNode,
    contribution::{GraphElement, GraphSupport},
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn object_ref(position: usize, role: u8) -> TestResult<ObjectRef> {
    let mut bytes = [role; 32];
    bytes
        .get_mut(..8)
        .ok_or("fixture digest prefix absent")?
        .copy_from_slice(&u64::try_from(position)?.to_be_bytes());
    Ok(ObjectRef(Digest(bytes)))
}

fn raw(subject: &RepoMapNodeRef) -> &str {
    match subject {
        RepoMapNodeRef::File(id) => id.as_str(),
        RepoMapNodeRef::Symbol(id) => id.as_str(),
        RepoMapNodeRef::Module(id) => id.as_str(),
        RepoMapNodeRef::Chunk(id) => id.as_str(),
    }
}

fn rows(count: usize) -> TestResult<Vec<CompiledRepoMapProjectionEntryV1>> {
    const TEXTS: &[&str] = &[
        "İstanbul",
        "Straße",
        "alpha needle",
        "한글 東京",
        "",
        "i\u{307}",
    ];
    const SCORES: &[i64] = &[0, 250, 500, 750, 1000];
    (0..count)
        .map(|position| {
            // Mixed lengths and variants deliberately differ from map-key order.
            let identity = if position.is_multiple_of(3) {
                format!("entry-{position:04}")
            } else {
                format!("entry-{position}")
            };
            let subject = if position.is_multiple_of(2) {
                file(&identity)
            } else {
                RepoMapNodeRef::Symbol(SymbolId::new(identity))
            };
            let text_index = position.checked_rem(TEXTS.len()).ok_or("empty text gold")?;
            let score_index = position
                .checked_rem(SCORES.len())
                .ok_or("empty score gold")?;
            let path_index = position.checked_rem(4).ok_or("zero path modulus")?;
            Ok(CompiledRepoMapProjectionEntryV1 {
                subject,
                doc_type: if position.is_multiple_of(2) {
                    RepoMapDocType::File
                } else {
                    RepoMapDocType::Symbol
                },
                owner_path: RepoRelativePath::new(format!("src/owner-{path_index}.rs")),
                search_text: TEXTS
                    .get(text_index)
                    .copied()
                    .ok_or("text gold absent")?
                    .to_owned(),
                final_score_millis: *SCORES.get(score_index).ok_or("score gold absent")?,
            })
        })
        .collect()
}

fn fixture(rows: &[CompiledRepoMapProjectionEntryV1]) -> TestResult<(Maps, Objects)> {
    let objects = Objects {
        values: Mutex::new(BTreeMap::new()),
    };
    let mut entries = Vec::new();
    for (position, row) in rows.iter().enumerate() {
        let row_ref = object_ref(position, 1)?;
        objects.insert(row_ref, row, &[])?;
        // Three supporters make an identity straddle the 128-entry scan page.
        for supporter in [1_u8, 2, 3] {
            let support_ref = object_ref(
                position,
                supporter.checked_add(1).ok_or("support role overflow")?,
            )?;
            let support = support(row, supporter, Some(row_ref));
            objects.insert(support_ref, &support, &[row_ref])?;
            entries.push((node_key(&row.subject, owner(supporter))?, support_ref));
        }
    }
    // Adjacency-only classes never become searchable candidates or drops.
    for (position, subject) in [
        RepoMapNodeRef::Module(RepoMapModuleId::new("module-only")),
        RepoMapNodeRef::Chunk(ChunkId::new("chunk-only")),
    ]
    .into_iter()
    .enumerate()
    {
        let support_ref = object_ref(position, 5)?;
        let support = GraphSupport {
            owner: owner(1),
            element: GraphElement::Node(GraphNode {
                identity: subject.clone(),
                payload_digest: Digest([9; 32]),
            }),
            projection: None,
        };
        objects.insert(support_ref, &support, &[])?;
        entries.push((node_key(&subject, owner(1))?, support_ref));
    }
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok((Maps { entries }, objects))
}

fn support(
    row: &CompiledRepoMapProjectionEntryV1,
    supporter: u8,
    projection: Option<ObjectRef>,
) -> GraphSupport {
    GraphSupport {
        owner: owner(supporter),
        element: GraphElement::Node(GraphNode {
            identity: row.subject.clone(),
            payload_digest: Digest([7; 32]),
        }),
        projection,
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Expected {
    hits: Vec<NativeRepoMapHit>,
    dropped: u32,
    drops: Vec<String>,
    degraded: Vec<String>,
}

fn full_sort(
    rows: &[CompiledRepoMapProjectionEntryV1],
    request: &NativeRepoMapQuery,
) -> Result<Expected, IndexError> {
    let terms: Vec<_> = tokenize(&request.query_text, CaseMode::Folded)
        .indexable()
        .map(|token| token.text.clone())
        .collect();
    let focus: BTreeSet<_> = request
        .focus_subjects
        .iter()
        .map(|key| (key.subject_identity.as_str(), key.subject_doc_type))
        .collect();
    let exact =
        |row: &CompiledRepoMapProjectionEntryV1| focus.contains(&(raw(&row.subject), row.doc_type));
    if focus.iter().any(|key| {
        !rows
            .iter()
            .any(|row| (raw(&row.subject), row.doc_type) == *key)
    }) {
        return Err(IndexError::FocusSubjectNotFound);
    }
    let paths: BTreeSet<_> = rows
        .iter()
        .filter(|row| exact(row))
        .map(|row| row.owner_path.as_str())
        .collect();
    let mut entries: Vec<_> = rows
        .iter()
        .filter(|row| focus.is_empty() || exact(row) || paths.contains(row.owner_path.as_str()))
        .map(|row| {
            let text = fold(&row.search_text);
            (
                row,
                terms
                    .iter()
                    .filter(|term| text.contains(term.as_str()))
                    .count(),
            )
        })
        .collect();
    let mut degraded = BTreeSet::new();
    if !terms.is_empty() && !entries.iter().any(|(_, matches)| *matches > 0) {
        let _inserted = degraded.insert("query_terms_unmatched");
    }
    entries.sort_by(|(left, left_matches), (right, right_matches)| {
        exact(right)
            .cmp(&exact(left))
            .then(
                paths
                    .contains(right.owner_path.as_str())
                    .cmp(&paths.contains(left.owner_path.as_str())),
            )
            .then(right_matches.cmp(left_matches))
            .then(right.final_score_millis.cmp(&left.final_score_millis))
            .then(raw(&left.subject).cmp(raw(&right.subject)))
    });
    let top_k = usize::try_from(request.top_k).map_err(|_error| IndexError::ResourceLimited)?;
    let mut hits = Vec::new();
    let mut consumed = 0_u32;
    let mut drops = BTreeSet::new();
    for (row, _) in &entries {
        let within_top_k = hits.len() < top_k;
        let next_tokens = consumed
            .checked_add(64)
            .ok_or(IndexError::ResourceLimited)?;
        let within_budget = next_tokens <= request.token_budget;
        let floor = hits.is_empty() && within_top_k && !within_budget;
        if within_top_k && (within_budget || floor) {
            if floor {
                let _inserted = degraded.insert("token_budget_floor_applied");
            }
            consumed = next_tokens;
            let rank = u32::try_from(hits.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            hits.push(NativeRepoMapHit {
                subject: row.subject.clone(),
                owner_path: row.owner_path.clone(),
                final_score_millis: u32::try_from(row.final_score_millis)
                    .map_err(|_error| IndexError::ResourceLimited)?,
                rank,
            });
        } else {
            let _inserted = drops.insert(if within_top_k {
                "token_budget_exhausted"
            } else {
                "top_k_exhausted"
            });
        }
    }
    let dropped = entries
        .len()
        .checked_sub(hits.len())
        .ok_or(IndexError::ResourceLimited)?;
    Ok(Expected {
        hits,
        dropped: u32::try_from(dropped).map_err(|_error| IndexError::ResourceLimited)?,
        drops: drops.into_iter().map(str::to_owned).collect(),
        degraded: degraded.into_iter().map(str::to_owned).collect(),
    })
}

fn focus(identity: &str, doc_type: RepoMapDocType) -> RepoMapFocusSubjectDto {
    RepoMapFocusSubjectDto {
        subject_identity: identity.into(),
        subject_doc_type: doc_type,
    }
}

#[test]
fn native_pages_agree_with_independent_full_sort_across_1890_requests() -> TestResult {
    let mut cases = 0_usize;
    let mut refusals = 0_usize;
    for count in [0, 7, 140] {
        let rows = rows(count)?;
        let (maps, objects) = fixture(&rows)?;
        let live = Arc::new(PageMemoryCount(std::sync::atomic::AtomicU64::new(0)));
        let focused = focus("entry-0000", RepoMapDocType::File);
        let other = focus("entry-1", RepoMapDocType::Symbol);
        let focus_cases = [
            Vec::new(),
            vec![focused.clone()],
            vec![other.clone()],
            vec![focused.clone(), other],
            vec![focused.clone(), focused.clone()],
            vec![focused, focus("absent", RepoMapDocType::File)],
            vec![focus("entry-0000", RepoMapDocType::Symbol)],
        ];
        for text in ["?", "i", "STRASSE", "pha", "東京 한글", "unmatched"] {
            for focus_subjects in &focus_cases {
                for top_k in [1, 7, 200] {
                    for token_budget in [1, 64, 127, 128, 10_000] {
                        let request = NativeRepoMapQuery {
                            query_text: text.into(),
                            focus_subjects: focus_subjects.clone(),
                            top_k,
                            token_budget,
                        };
                        validate_request(&request)?;
                        let expected = full_sort(&rows, &request);
                        let mut work = budget()?;
                        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(PageMemoryScope(
                            Arc::clone(&live),
                        )))));
                        let actual = query_rank(
                            MapRoot(Some(reference(100))),
                            &request,
                            &maps,
                            &objects,
                            &mut work,
                        );
                        match (expected, actual) {
                            (Ok(expected), Ok(page)) => {
                                let actual = Expected {
                                    hits: page.hits().to_vec(),
                                    dropped: page.dropped_entries_count,
                                    drops: page.drop_reason_codes().to_vec(),
                                    degraded: page.degraded_reason_codes().to_vec(),
                                };
                                if actual != expected {
                                    return Err(format!("native parity: count={count}, {request:?}, expected={expected:?}, actual={actual:?}").into());
                                }
                                if !page.hits().is_empty() && live.0.load(Ordering::SeqCst) == 0 {
                                    return Err("native parity page lost memory custody".into());
                                }
                            }
                            (
                                Err(IndexError::FocusSubjectNotFound),
                                Err(IndexError::FocusSubjectNotFound),
                            ) => {
                                refusals =
                                    refusals.checked_add(1).ok_or("refusal count overflow")?;
                            }
                            (expected, actual) => {
                                let actual_error = actual.err();
                                return Err(format!(
                                    "native parity error: count={count}, {request:?}, expected={expected:?}, actual_error={actual_error:?}"
                                ).into());
                            }
                        }
                        drop(work);
                        if live.0.load(Ordering::SeqCst) != 0 {
                            return Err("native parity request leaked memory custody".into());
                        }
                        cases = cases.checked_add(1).ok_or("case count overflow")?;
                    }
                }
            }
        }
    }
    if cases != 1890 || refusals != 900 {
        return Err(format!("matrix coverage drifted: {cases} cases/{refusals} refusals").into());
    }
    Ok(())
}

fn require_corruption(maps: &Maps, objects: &Objects) -> TestResult {
    let live = Arc::new(PageMemoryCount(std::sync::atomic::AtomicU64::new(0)));
    let mut work = budget()?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(PageMemoryScope(
        Arc::clone(&live),
    )))));
    let request = NativeRepoMapQuery {
        query_text: "needle".into(),
        focus_subjects: Vec::new(),
        top_k: 7,
        token_budget: 128,
    };
    let result = query_rank(
        MapRoot(Some(reference(100))),
        &request,
        maps,
        objects,
        &mut work,
    );
    if !matches!(result, Err(IndexError::CorruptObject(_))) {
        return Err(format!("corrupt rank state did not refuse: {:?}", result.err()).into());
    }
    drop(work);
    if live.0.load(Ordering::SeqCst) != 0 {
        return Err("corrupt rank query leaked memory custody".into());
    }
    Ok(())
}

#[test]
fn missing_projection_on_one_surviving_supporter_refuses_and_refunds() -> TestResult {
    let rows = rows(1)?;
    let row = rows.first().ok_or("missing fault fixture row")?;
    let (maps, objects) = fixture(&rows)?;
    objects.insert(object_ref(0, 3)?, &support(row, 2, None), &[])?;
    require_corruption(&maps, &objects)
}

#[test]
fn disagreeing_projection_facts_refuse_and_refund() -> TestResult {
    let rows = rows(1)?;
    let row = rows.first().ok_or("missing fault fixture row")?;
    let (maps, objects) = fixture(&rows)?;
    let mut different = row.clone();
    different.search_text = "changed needle".into();
    let changed_ref = object_ref(9999, 1)?;
    objects.insert(changed_ref, &different, &[])?;
    objects.insert(
        object_ref(0, 3)?,
        &support(row, 2, Some(changed_ref)),
        &[changed_ref],
    )?;
    require_corruption(&maps, &objects)
}

#[test]
fn cross_variant_raw_identity_collision_refuses_and_refunds() -> TestResult {
    let mut rows = rows(2)?;
    let shared_identity =
        raw(&rows.first().ok_or("missing first collision row")?.subject).to_owned();
    let second = rows.get_mut(1).ok_or("missing second collision row")?;
    second.subject = RepoMapNodeRef::Symbol(SymbolId::new(shared_identity));
    let (maps, objects) = fixture(&rows)?;
    require_corruption(&maps, &objects)
}
