use quanta_index_contract::{
    BundleEncoding, BundleMode, ManifestGeneration, PublishedGenerationSet, RepoId, RevisionId,
};
use quanta_index_core::CoreError;
use rusqlite::Row;

pub(super) const fn mode_to_str(mode: BundleMode) -> &'static str {
    match mode {
        BundleMode::ServeOnly => "serve_only",
        BundleMode::IndexBuild => "index_build",
    }
}

pub(super) const fn encoding_to_str(encoding: BundleEncoding) -> &'static str {
    match encoding {
        BundleEncoding::ArrowIpc => "arrow_ipc",
        BundleEncoding::Feather => "feather",
        BundleEncoding::Json => "json",
        BundleEncoding::RawF32 => "raw_f32",
        BundleEncoding::TantivyDirectory => "tantivy_directory",
        BundleEncoding::LanceDirectory => "lance_directory",
        BundleEncoding::Opaque => "opaque",
    }
}

pub(super) fn row_to_generation_set(row: &Row<'_>) -> rusqlite::Result<PublishedGenerationSet> {
    Ok(PublishedGenerationSet {
        repo_id: RepoId::new(row.get::<_, String>(0)?),
        revision_id: RevisionId::new(row.get::<_, String>(1)?),
        manifest_generation: ManifestGeneration::new(row.get::<_, u64>(2)?),
        lexical_generation: quanta_index_contract::GenerationId::new(row.get::<_, u64>(3)?),
        symbol_generation: quanta_index_contract::GenerationId::new(row.get::<_, u64>(4)?),
        structural_generation: row
            .get::<_, Option<u64>>(5)?
            .map(quanta_index_contract::GenerationId::new),
        history_generation: row
            .get::<_, Option<u64>>(6)?
            .map(quanta_index_contract::GenerationId::new),
        semantic_generation: row
            .get::<_, Option<u64>>(7)?
            .map(quanta_index_contract::GenerationId::new),
        metadata_generation: row
            .get::<_, Option<u64>>(8)?
            .map(quanta_index_contract::GenerationId::new),
    })
}

#[doc(hidden)]
pub fn sqlite_u64_to_i64(field: &str, value: u64) -> Result<i64, CoreError> {
    i64::try_from(value).map_err(|error| {
        CoreError::Storage(format!("{field} exceeds sqlite INTEGER range: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{BundleEncoding, BundleMode};

    use super::{encoding_to_str, mode_to_str, sqlite_u64_to_i64};

    #[test]
    fn maps_bundle_modes_to_sql_strings() {
        assert_eq!(mode_to_str(BundleMode::ServeOnly), "serve_only");
        assert_eq!(mode_to_str(BundleMode::IndexBuild), "index_build");
    }

    #[test]
    fn maps_bundle_encodings_to_sql_strings() {
        assert_eq!(encoding_to_str(BundleEncoding::Json), "json");
        assert_eq!(encoding_to_str(BundleEncoding::ArrowIpc), "arrow_ipc");
    }

    #[test]
    fn rejects_values_outside_sqlite_integer_range() {
        let result = sqlite_u64_to_i64("value", u64::MAX);
        assert!(result.is_err(), "unexpected result: {result:?}");
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("exceeds sqlite INTEGER range"))
        );
    }
}
