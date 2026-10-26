//! Selected path lookup and shared source-to-projection candidate selection.

use super::*;

pub(super) fn validate_path_roots(
    source_owners: MapRoot,
    source_paths: MapRoot,
) -> Result<(), IndexError> {
    // Source publication installs one canonical path entry per source owner.
    // A missing path root beside a nonempty owner root cannot prove an empty
    // eligible set; reject before embedding as well as before ranking.
    if source_owners.0.is_some() != source_paths.0.is_some() {
        return Err(IndexError::CorruptObject(
            "selected source owner/path root presence differs".into(),
        ));
    }
    Ok(())
}

/// Resolve complete selected-source path membership before ANN candidates.
/// Projected members use the same persisted reverse layout as lexical scope.
pub(super) fn search(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<Option<SearchResult>, IndexError> {
    let Some(path) = query.exact_path else {
        return Ok(None);
    };
    if !matches!(query.coverage.status, CoverageStatus::Complete) {
        return Ok(None);
    }
    if let Some(projection) = query.projection.as_ref() {
        // The standalone exact layout has no derived reverse directory. Its
        // existing bounded exact traversal retains truthful cap semantics.
        if query.coverage.searchable == query.coverage.owners.0
            && !lexical_scope::selected_projected_empty(query.coverage)
        {
            return Ok(None);
        }
        return search_projected(query, projection, path, context).map(Some);
    }
    let query_norm = validate_search_query(query)?;
    validate_path_roots(query.source_owners, query.source_paths)?;
    let top_k = usize::try_from(query.top_k).map_err(|_error| IndexError::ResourceLimited)?;
    let overlay_rows = u32::try_from(
        query
            .overlay
            .as_ref()
            .map_or(0, |value| value.documents.len()),
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    let durable_limit = query
        .max_owners
        .checked_sub(overlay_rows)
        .ok_or(IndexError::ResourceLimited)?;
    // The public exact-path request permits a few more bytes than a Native
    // `SourceCut` path key. Such a path has no durable source member; an admitted
    // overlay with that path still participates below.
    let source_owner = if path.len() > MAX_SOURCE_PATH_BYTES {
        None
    } else {
        let path_key = SourcePathKey::copy(path, context.budget)?;
        let owner = context
            .maps
            .lookup(query.source_paths, path_key.as_slice(), context.budget)?
            .map(|value| owner_from_path_map_value(value, context.budget))
            .transpose()?;
        drop(path_key);
        owner
    };
    let heap_bytes = top_k
        .checked_mul(std::mem::size_of::<RankedHit>())
        .ok_or(IndexError::ResourceLimited)?;
    let _heap_memory = context.budget.reserve_memory(
        u64::try_from(heap_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut heap = BinaryHeap::new();
    heap.try_reserve_exact(top_k)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if heap.capacity() > top_k {
        return Err(IndexError::ResourceLimited);
    }
    let mut scanned_owners = 0;
    let mut skipped_stale = 0;
    let mut traversal = NativeSemanticTraversal::ExactExhausted;
    if let Some(source_owner) = source_owner {
        let key = owner_map_key(source_owner);
        let source = context
            .maps
            .lookup_object(query.source_owners, &key, context.budget)?
            .ok_or_else(|| IndexError::CorruptObject("source path has no selected owner".into()))?;
        let record = load_source_record_from_objects(source, context.objects, context.budget)?;
        if record.path != path {
            return Err(IndexError::CorruptObject(
                "source path reverse record differs".into(),
            ));
        }
        let coverage = context
            .maps
            .lookup(query.coverage.owners, &key, context.budget)?
            .ok_or_else(|| {
                IndexError::CorruptObject("complete semantic coverage omitted path owner".into())
            })?;
        let selected = selected_owner::SelectedOwner::load(&key, coverage, query, context)?;
        if selected.coverage.desired.source != Some(source) {
            return Err(IndexError::CorruptObject(
                "selected semantic source differs from path owner".into(),
            ));
        }
        if durable_limit == 0 {
            // Authenticate the direct path and coverage links, but do not
            // inspect a vector candidate after the durable owner cap is spent.
            // As in search_exact, authority probes do not count as scanned owners.
            traversal = NativeSemanticTraversal::ExactCapped;
        } else {
            scanned_owners = 1;
            if selected.coverage.covered != Some(selected.coverage.desired)
                || selected.coverage.searchable.is_none()
            {
                skipped_stale = 1;
            } else {
                let member = selected.current_member(query, context)?;
                if !selected.matches_exact_path(query, context)? {
                    return Err(IndexError::CorruptObject(
                        "selected semantic path differs from source path".into(),
                    ));
                }
                rank_member(
                    &member,
                    selected.contribution,
                    None,
                    query,
                    query_norm,
                    top_k,
                    &mut heap,
                    context,
                )?;
            }
        }
    }
    finish_ranked(
        query,
        query_norm,
        top_k,
        heap,
        scanned_owners,
        skipped_stale,
        traversal,
        context,
    )
    .map(Some)
}

fn search_projected(
    query: &SearchQuery<'_>,
    projection: &ProjectionBinding,
    path: &str,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    validate_path_roots(query.source_owners, query.source_paths)?;
    let overlay_count = u32::try_from(
        query
            .overlay
            .as_ref()
            .map_or(0, |overlay| overlay.documents.len()),
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    if lexical_scope::selected_projected_empty(query.coverage) {
        return lexical_scope::rank_selected(query, &[], overlay_count, context);
    }
    let directory =
        lexical_scope::selected_projected_directory(query.coverage, query.recipe, context)?;
    let limit = query
        .max_owners
        .checked_sub(overlay_count)
        .and_then(|value| value.checked_add(1))
        .ok_or(IndexError::ResourceLimited)?;
    let source_owner = if path.len() > MAX_SOURCE_PATH_BYTES {
        None
    } else {
        let key = SourcePathKey::copy(path, context.budget)?;
        context
            .maps
            .lookup(query.source_paths, key.as_slice(), context.budget)?
            .map(|value| owner_from_path_map_value(value, context.budget))
            .transpose()?
    };
    let Some(source_owner) = source_owner else {
        return lexical_scope::rank_selected(query, &[], overlay_count, context);
    };
    let mut selected = lexical_scope::SelectedLexicalScope::reserve(
        usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
        context,
    )?;
    {
        let source = context
            .maps
            .lookup_object(
                query.source_owners,
                &owner_map_key(source_owner),
                context.budget,
            )?
            .ok_or_else(|| IndexError::CorruptObject("source path has no selected owner".into()))?;
        let record = load_source_record_from_objects(source, context.objects, context.budget)?;
        if record.path != path {
            return Err(IndexError::CorruptObject(
                "source path reverse record differs".into(),
            ));
        }
        lexical_scope::select_projected_source(
            source_owner,
            source,
            Some(path),
            query.coverage,
            projection,
            query.source_owners,
            directory.source_reverse,
            usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
            &mut selected,
            context,
        )?;
    }
    lexical_scope::rank_selected(query, &selected.owners, overlay_count, context)
}
