//! Search layout preparation owned by the original owner-publication attempt.

use quanta_index_core::index::ports::ObjectMapBatchPort;
use quanta_index_core::index::{
    AttemptContext, CoverageBinding, IndexError, InputDomain, InstanceSpec, MapRoot,
    MemoryLifetime, ObjectRef, OwnerKey,
};
use std::sync::Arc;

use super::ann_directory::{
    AnnDirectory, MAX_DELTA_OWNERS, RegionDescriptor, ValidatedDeltaMember,
};
use super::{EmbeddingRecipe, ReadContext};
use crate::native::{
    ann_files, ann_lance, ann_session::AnnLanceSession, ann_store::AnnStoreLimits,
};

fn limited<T>(_error: T) -> IndexError {
    IndexError::ResourceLimited
}

pub(super) async fn prepare(
    instance: &InstanceSpec,
    recipe: &EmbeddingRecipe,
    prior: Option<&CoverageBinding>,
    owners: MapRoot,
    materialized: Option<(OwnerKey, ObjectRef)>,
    object_maps: Option<&Arc<dyn ObjectMapBatchPort>>,
    context: &mut AttemptContext<'_>,
) -> Result<quanta_index_core::index::SearchablePreparation, IndexError> {
    context.budget.checkpoint()?;
    let prior_directory = prior.and_then(|coverage| coverage.searchable);
    let projected = matches!(instance.input_domain, InputDomain::Projection { .. });
    let mut directory = if let Some(reference) = prior_directory {
        // An owner-map layout is a different persisted format and must rebuild.
        if prior.is_some_and(|coverage| coverage.searchable == coverage.owners.0) {
            return Err(IndexError::RebuildRequired);
        }
        AnnDirectory::load(
            reference,
            recipe.fingerprint(),
            &mut ReadContext {
                objects: context.objects,
                maps: context.maps,
                budget: context.budget,
            },
        )?
    } else {
        // With no prior directory there is no complete reverse index. Build it
        // from the whole selected owner root, including unchanged prior
        // coverage, before this new directory can become a Catalog child.
        AnnDirectory::empty(
            recipe.fingerprint(),
            if projected {
                MapRoot(None)
            } else {
                prior.map_or(owners, |coverage| coverage.owners)
            },
        )
    };
    if prior_directory.is_some()
        && prior.is_some_and(|coverage| coverage.owners != directory.owners)
    {
        return Err(IndexError::CorruptObject(
            "ANN prior directory owner root mismatch".into(),
        ));
    }
    if projected {
        directory.refresh_source_reverse(instance, owners, context)?;
    } else if directory.source_reverse.0.is_some() {
        return Err(IndexError::CorruptObject(
            "SourceLocal ANN directory contains projected reverse membership".into(),
        ));
    }
    directory.owners = owners;
    if owners.0.is_none() {
        if materialized.is_some() {
            return Err(IndexError::CorruptObject(
                "ANN materialized member has no selected owner".into(),
            ));
        }
        // Current empty coverage no longer retains obsolete regions; old views
        // still own their previous directory and complete physical child graph.
        directory.regions = MapRoot(None);
        directory.region_count = 0;
        directory.maintenance_after = None;
        directory.delta = MapRoot(None);
        directory.delta_owners = 0;
        directory.delta_rows = 0;
        return directory.prepare_searchable(context);
    }
    if materialized.is_none() {
        directory.retire_delta(recipe, context)?;
    }
    directory.retire_obsolete(context)?;
    let mut prepared_delta = None;
    if let Some((owner, reference)) = materialized {
        let mut validated = ValidatedDeltaMember::load(reference, owner, recipe, context)?;
        if validated.binding().instance != instance.instance
            || validated.binding().format != instance.format
        {
            return Err(IndexError::CorruptObject(
                "ANN materialized member instance mismatch".into(),
            ));
        }
        let rows = validated.rows()?;
        if rows == 0 {
            // Empty output does not enter the delta; release its decode lease
            // before loading an older member or saving the directory.
            drop(validated);
            directory.remove_delta(owner, recipe, context)?;
            return directory.prepare_searchable(context);
        }
        let row_limit = u32::try_from(ann_lance::MAX_REGION_CELLS)
            .map_err(limited)?
            .checked_div(recipe.dimension)
            .ok_or(IndexError::ResourceLimited)?
            .min(u32::try_from(ann_lance::MAX_REGION_ROWS).map_err(limited)?);
        // Seal before adding an owner that would exceed the region envelope.
        // Replacements are conservatively sealed rather than undercounted.
        if directory.delta_owners != 0
            && (directory.delta_owners == MAX_DELTA_OWNERS
                || directory
                    .delta_rows
                    .checked_add(rows)
                    .is_none_or(|total| total > row_limit))
        {
            // Release the new owner's decode lease before sealing the prior
            // region. Revalidate afterward; non-seal owners retain custody.
            drop(validated);
            seal_delta(&mut directory, recipe, context).await?;
            validated = ValidatedDeltaMember::load(reference, owner, recipe, context)?;
            if validated.rows()? != rows
                || validated.binding().instance != instance.instance
                || validated.binding().format != instance.format
            {
                return Err(IndexError::CorruptObject(
                    "ANN materialized member changed during seal".into(),
                ));
            }
        }
        if let Some(prepared) = directory.prepare_validated_delta(validated, recipe, context)? {
            // A sealed delta must be readable by the Lance builder. Only the
            // non-seal edit may remain resident until its descriptor and
            // coverage parent join the original child-first installation.
            let will_seal = prepared.next_owners == MAX_DELTA_OWNERS || prepared.next_rows >= 256;
            if !will_seal && let Some(writer) = object_maps {
                let token = Arc::clone(writer).prepare_one_map(
                    directory.delta,
                    &prepared.edit,
                    context.attempt,
                    context.budget,
                )?;
                directory.delta = token.root();
                directory.delta_owners = prepared.next_owners;
                directory.delta_rows = prepared.next_rows;
                prepared_delta = Some(token);
            } else {
                directory.delta =
                    context.apply(directory.delta, std::slice::from_ref(&prepared.edit))?;
                directory.delta_owners = prepared.next_owners;
                directory.delta_rows = prepared.next_rows;
            }
        }
        if directory.delta_owners == MAX_DELTA_OWNERS || directory.delta_rows >= 256 {
            seal_delta(&mut directory, recipe, context).await?;
        }
    }
    let searchable = directory.prepare_searchable(context)?;
    if let Some(token) = prepared_delta {
        searchable.with_prepared_map(token, context)
    } else {
        Ok(searchable)
    }
}

async fn seal_delta(
    directory: &mut AnnDirectory,
    recipe: &EmbeddingRecipe,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if directory.delta_rows == 0 {
        return Ok(());
    }
    let _runtime = tokio::runtime::Handle::try_current().map_err(|_error| {
        IndexError::BackendViolation("Native ANN publication requires a Tokio runtime".into())
    })?;
    // Coalesce small runs in this same COW attempt before checking net capacity.
    // Failed staging never publishes these removals. No intermediate Lance build.
    let members = directory.consolidated_members(recipe, context)?;
    if members.rows == 0 {
        directory.delta = MapRoot(None);
        directory.delta_owners = 0;
        directory.delta_rows = 0;
        return Ok(());
    }
    directory.ensure_region_capacity(context)?;
    let max_owners = usize::try_from(members.owners).map_err(limited)?;
    let row_count = usize::try_from(members.rows).map_err(limited)?;
    let dimension = usize::try_from(recipe.dimension).map_err(limited)?;
    let backing = row_count
        .checked_mul(std::mem::size_of::<ann_lance::RegionRow<'_>>())
        .and_then(|bytes| bytes.checked_add(dimension.checked_mul(4)?))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = context.budget.reserve_memory(
        u64::try_from(backing).map_err(limited)?,
        MemoryLifetime::Temporary,
    )?;
    // Consolidation has already authenticated each member and its artifact
    // while determining the exact row count. Keep that attempt-local custody
    // through sealing; opening every artifact again doubles carry work.
    let artifacts = &members.artifacts;
    if artifacts.len() != max_owners {
        return Err(IndexError::CorruptObject(
            "ANN region owner count mismatch".into(),
        ));
    }
    let mut after: Option<[u8; 38]> = None;
    let mut inspected = 0_usize;
    loop {
        let page = context.maps.scan(
            members.root,
            after.as_ref().map(<[u8; 38]>::as_slice),
            MAX_DELTA_OWNERS,
            context.budget,
        )?;
        if page.entries.len() > usize::try_from(MAX_DELTA_OWNERS).map_err(limited)?
            || (page.entries.is_empty() && page.has_more)
        {
            return Err(IndexError::BackendViolation(
                "ANN region member scan made no bounded progress".into(),
            ));
        }
        for (key, reference) in &page.entries {
            context.budget.checkpoint()?;
            let reference = reference.as_object_ref()?;
            let cursor: [u8; 38] = key.as_slice().try_into().map_err(|_error| {
                IndexError::CorruptObject("ANN region member key width differs".into())
            })?;
            if after.is_some_and(|prior| cursor <= prior) || inspected >= max_owners {
                return Err(IndexError::CorruptObject(
                    "ANN region member order/count differs".into(),
                ));
            }
            after = Some(cursor);
            let selected = artifacts.get(inspected).ok_or_else(|| {
                IndexError::CorruptObject("ANN region member count differs".into())
            })?;
            if *reference != selected.reference
                || key.as_slice()
                    != quanta_index_core::index::owner_map_key(selected.binding.owner).as_slice()
            {
                return Err(IndexError::CorruptObject(
                    "ANN region map differs from authenticated member".into(),
                ));
            }
            inspected = inspected
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        if !page.has_more {
            break;
        }
    }
    if inspected != max_owners {
        return Err(IndexError::CorruptObject(
            "ANN region owner count mismatch".into(),
        ));
    }
    let mut rows = Vec::new();
    rows.try_reserve_exact(row_count).map_err(limited)?;
    let mut center = Vec::new();
    center.try_reserve_exact(dimension).map_err(limited)?;
    center.resize(dimension, 0.0f32);
    for selected in artifacts {
        for vector in &selected.artifact.vectors {
            if rows.len() == row_count {
                return Err(IndexError::CorruptObject(
                    "ANN delta row count exceeded".into(),
                ));
            }
            for (sum, value) in center.iter_mut().zip(&vector.values) {
                *sum += *value;
            }
            rows.push(ann_lance::RegionRow {
                owner: selected.binding.owner,
                member: selected.binding.logical_commitment,
                vector_id: &vector.id,
                input: selected.binding.input,
                vector: &vector.values,
            });
        }
    }
    if rows.len() != row_count {
        return Err(IndexError::CorruptObject(
            "ANN delta row count differs".into(),
        ));
    }
    if center.iter().any(|value| !value.is_finite()) || center.iter().all(|value| *value == 0.0) {
        let first = rows
            .first()
            .ok_or_else(|| IndexError::CorruptObject("ANN delta unexpectedly empty".into()))?;
        center.copy_from_slice(first.vector);
    }
    rows.sort_by(|a, b| (a.owner, a.member, a.vector_id).cmp(&(b.owner, b.member, b.vector_id)));
    // This is a cumulative staging quota, not an assertion about Lance RSS.
    // The facade refuses before exceeding it; it never raises original funding.
    let row_bytes = rows.iter().try_fold(0u64, |sum, row| {
        let vectors = u64::try_from(row.vector.len())
            .map_err(limited)?
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?;
        let id = u64::try_from(row.vector_id.len()).map_err(limited)?;
        sum.checked_add(vectors)
            .and_then(|value| value.checked_add(id))
            .and_then(|value| value.checked_add(256))
            .ok_or(IndexError::ResourceLimited)
    })?;
    let write_quota = row_bytes
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(1_048_576))
        .ok_or(IndexError::ResourceLimited)?
        .min(33_554_432);
    let limits = AnnStoreLimits {
        max_write_bytes: write_quota,
        max_read_bytes: write_quota.saturating_mul(4).min(67_108_864),
        ..AnnStoreLimits::default()
    };
    let session = AnnLanceSession::new(limits, context.budget)?;
    let connection = session.connect().await?;
    let generation = directory
        .next_region
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let build_span = crate::build::begin_build_stage(
        context.budget.request().diagnostic_context(),
        quanta_index_core::DiagnosticStageV1::SemanticBuild,
    );
    let built = ann_lance::build_region(
        &connection,
        "region",
        &rows,
        recipe.fingerprint(),
        generation,
        dimension,
        context.budget,
    )
    .await;
    let resolved = session.resolve(built);
    crate::build::finish_build_stage(build_span, resolved.is_ok());
    let (table, seal) = resolved?;
    drop(table);
    drop(connection);
    let files = session.freeze_and_snapshot().await?;
    let _inputs_memory = context.budget.reserve_memory(
        u64::try_from(
            files
                .len()
                .checked_mul(std::mem::size_of::<ann_files::AnnFileInput<'_>>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(limited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut inputs = Vec::new();
    inputs.try_reserve_exact(files.len()).map_err(limited)?;
    for file in &files {
        inputs.push(ann_files::AnnFileInput {
            path: &file.path,
            bytes: &file.bytes,
        });
    }
    let fileset = ann_files::install_fileset(context, seal, &inputs)?;
    let _region = directory.append_region(
        &RegionDescriptor {
            fileset,
            members: members.root,
            center,
            rows: members.rows,
        },
        context,
    )?;
    directory.delta = MapRoot(None);
    directory.delta_owners = 0;
    directory.delta_rows = 0;
    Ok(())
}
