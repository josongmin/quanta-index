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
    directory: Option<&ann_directory::AnnDirectory>,
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
        return search_projected(query, projection, path, directory, context).map(Some);
    }
    let query_norm = validate_search_query(query)?;
    validate_path_roots(query.source_owners, query.source_paths)?;
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
    let source_path = if path.len() > MAX_SOURCE_PATH_BYTES {
        None
    } else {
        let path_key = SourcePathKey::copy(path, context.budget)?;
        let owner = context
            .maps
            .lookup(query.source_paths, path_key.as_slice(), context.budget)?
            .map(|value| quanta_index_core::index::SourcePathOwners::decode(value, context.budget))
            .transpose()?;
        drop(path_key);
        owner
    };
    let mut heap = RankedHits::for_query(query, context.budget)?;
    let mut scanned_owners = 0;
    let mut skipped_stale = 0u32;
    let mut traversal = NativeSemanticTraversal::ExactExhausted;
    let mut rank_source = |source_owner: OwnerKey,
                           source: ObjectRef,
                           record: SourceRecordOwner,
                           context: &mut ReadContext<'_>|
     -> Result<(), IndexError> {
        let key = owner_map_key(source_owner);
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
        if scanned_owners == durable_limit {
            // Authenticate the direct path and coverage links, but do not
            // inspect a vector candidate after the durable owner cap is spent.
            // As in search_exact, authority probes do not count as scanned owners.
            traversal = NativeSemanticTraversal::ExactCapped;
        } else {
            scanned_owners = scanned_owners
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if selected.coverage.covered != Some(selected.coverage.desired)
                || selected.coverage.searchable.is_none()
            {
                skipped_stale = skipped_stale
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            } else {
                let member = selected.current_member(query, context)?;
                let _rescored = rank_member(
                    &selected,
                    &member,
                    Some(SelectedArtifactInput::Source {
                        reference: source,
                        record,
                    }),
                    None,
                    query,
                    query_norm,
                    &mut heap,
                    context,
                )?;
            }
        }
        Ok(())
    };
    match source_path {
        None => {}
        Some(quanta_index_core::index::SourcePathOwners::LegacyOwner(source_owner)) => {
            let source = context
                .maps
                .lookup_object(
                    query.source_owners,
                    &owner_map_key(source_owner),
                    context.budget,
                )?
                .ok_or_else(|| {
                    IndexError::CorruptObject("source path has no selected owner".into())
                })?;
            let record = load_source_record_with_lifetime(
                source,
                context.objects,
                context.budget,
                if query.source_files && durable_limit > 0 {
                    MemoryLifetime::Retained
                } else {
                    MemoryLifetime::Temporary
                },
            )?;
            if record.path != path {
                return Err(IndexError::CorruptObject(
                    "source path reverse record differs".into(),
                ));
            }
            rank_source(source_owner, source, record, context)?;
        }
        Some(quanta_index_core::index::SourcePathOwners::Repositories(root)) => {
            let maps = context.maps;
            let objects = context.objects;
            quanta_index_core::index::visit_repository_path_sources(
                root,
                path,
                query.source_owners,
                maps,
                objects,
                context.budget,
                &mut |owner, source, record, budget| {
                    let mut read = ReadContext {
                        objects,
                        maps,
                        budget,
                    };
                    rank_source(owner, source, record, &mut read)
                },
            )?;
        }
    }
    drop(rank_source);
    finish_ranked(
        query,
        query_norm,
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
    directory: Option<&ann_directory::AnnDirectory>,
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
        return lexical_scope::rank_selected(
            query,
            lexical_scope::SelectedLexicalScope::new(0, overlay_count),
            None,
            context,
        );
    }
    let directory = directory.ok_or(IndexError::RebuildRequired)?;
    let limit = usize::try_from(
        query
            .max_owners
            .checked_sub(overlay_count)
            .and_then(|value| value.checked_add(1))
            .ok_or(IndexError::ResourceLimited)?,
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    let source_path = if path.len() > MAX_SOURCE_PATH_BYTES {
        None
    } else {
        let key = SourcePathKey::copy(path, context.budget)?;
        context
            .maps
            .lookup(query.source_paths, key.as_slice(), context.budget)?
            .map(|value| quanta_index_core::index::SourcePathOwners::decode(value, context.budget))
            .transpose()?
    };
    let Some(source_path) = source_path else {
        return lexical_scope::rank_selected(
            query,
            lexical_scope::SelectedLexicalScope::new(0, overlay_count),
            None,
            context,
        );
    };
    let mut selected = lexical_scope::SelectedLexicalScope::new(limit, overlay_count);
    let mut scalar_file = None;
    match source_path {
        quanta_index_core::index::SourcePathOwners::LegacyOwner(source_owner) => {
            let source = lexical_scope::SelectedProjectedSource::from_path(
                source_owner,
                path,
                query.source_owners,
                query.source_files,
                context,
            )?;
            lexical_scope::select_projected_source(
                &source,
                query.coverage,
                projection,
                query.source_owners,
                directory.source_reverse,
                &mut selected,
                query.source_files,
                context,
            )?;
            scalar_file = source.file_record;
        }
        quanta_index_core::index::SourcePathOwners::Repositories(root) => {
            let maps = context.maps;
            let objects = context.objects;
            quanta_index_core::index::visit_repository_path_sources(
                root,
                path,
                query.source_owners,
                maps,
                objects,
                context.budget,
                &mut |owner, reference, record, budget| {
                    let mut read = ReadContext {
                        objects,
                        maps,
                        budget,
                    };
                    // The common bucket traversal authenticated this record.
                    // File hits reopen their selected source at ranking so one
                    // record cannot be incorrectly shared by every projection.
                    let source = lexical_scope::SelectedProjectedSource::from_authenticated(
                        owner,
                        reference,
                        query.source_owners,
                        record,
                        false,
                        &mut read,
                    )?;
                    lexical_scope::select_projected_source(
                        &source,
                        query.coverage,
                        projection,
                        query.source_owners,
                        directory.source_reverse,
                        &mut selected,
                        query.source_files,
                        &mut read,
                    )
                },
            )?;
        }
    }
    lexical_scope::rank_selected(query, selected, scalar_file, context)
}
