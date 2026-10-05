//! Exact live BM25 corpus statistics for a committed lexical generation.
//!
//! Tantivy's default provider includes deleted documents in N, df, and
//! token totals. A seal persists exact per-segment statistics. A carried
//! segment inherits its committed statistics; new deaths are subtracted from
//! the stored producer census of just those documents. Query weights never
//! repair corpus statistics.

#![expect(
    clippy::redundant_pub_crate,
    reason = "this module is private to the lexical crate"
)]

use std::collections::{BTreeMap, btree_map::Entry};
use std::path::Path;
use std::time::Instant;

use quanta_index_core::CoreError;
use tantivy::query::Bm25StatisticsProvider;
use tantivy::schema::{Field, TantivyDocument, Value as _};
use tantivy::{DocAddress, Index, Searcher, SegmentComponent, SegmentReader, Term};

use crate::SchemaFields;
use crate::doc_census::DocCensus;
use quanta_index_core::domains::generation::SealedArtifactCommitmentV1;

pub(crate) const FILE_NAME: &str = "search-corpus-live-bm25.cbor";
/// Reuse the committed index-control-file admission ceiling.
pub(crate) const MAX_BYTES: usize = super::index_directory::MAX_INDEX_CONTROL_BYTES;
/// Four decoded bytes per admitted control byte for retained statistics.
/// Decode also holds a bounded wire row; transient peak needs measurement.
const MAX_RESIDENT: usize = MAX_BYTES * 4;
mod codec;
type SegmentIdentity = (String, u32, u32, Option<u64>);
pub(crate) struct BaseSnapshot {
    pub(super) statistics: LiveBm25Statistics,
    pub(super) index: Index,
    pub(super) index_segments: Vec<SealedArtifactCommitmentV1>,
}

#[derive(Clone, Debug)]
struct SegmentStatistics {
    identity: SegmentIdentity,
    field_tokens: BTreeMap<u32, u64>,
    dead_df: BTreeMap<(u32, Vec<u8>), u64>,
}

pub(crate) struct LiveBm25Statistics {
    index_meta_digest: [u8; 32],
    live_docs: u64,
    segments: Vec<SegmentStatistics>,
    field_tokens: BTreeMap<u32, u64>,
    encoded_bytes: usize,
}

/// One bounded marker per statistics build, with nested call envelopes.
/// Stored census lengths are logical payload bytes, not filesystem read bytes.
struct BuildObservation {
    started: Option<Instant>,
    reused_segments: u64,
    changed_segments: u64,
    new_segments: u64,
    mask_docs: u64,
    new_census_docs: u64,
    newly_dead_docs: u64,
    logical_census_bytes: u64,
    changed_ns: u64,
    new_ns: u64,
    death_ns: u64,
    invalid: bool,
}

impl BuildObservation {
    fn new() -> Self {
        Self {
            started: crate::causal_profile::enabled().then(Instant::now),
            reused_segments: 0,
            changed_segments: 0,
            new_segments: 0,
            mask_docs: 0,
            new_census_docs: 0,
            newly_dead_docs: 0,
            logical_census_bytes: 0,
            changed_ns: 0,
            new_ns: 0,
            death_ns: 0,
            invalid: false,
        }
    }

    fn add_counter(total: &mut u64, amount: Option<u64>, invalid: &mut bool) {
        if let Some(next) = amount.and_then(|amount| total.checked_add(amount)) {
            *total = next;
        } else {
            *invalid = true;
        }
    }

    fn add_bytes(&mut self, bytes: usize) {
        let Ok(amount) = u64::try_from(bytes) else {
            self.invalid = true;
            return;
        };
        Self::add_counter(
            &mut self.logical_census_bytes,
            Some(amount),
            &mut self.invalid,
        );
    }

    fn add_elapsed(total: &mut u64, started: Option<Instant>, invalid: &mut bool) {
        if let Some(started) = started {
            let Ok(amount) = u64::try_from(started.elapsed().as_nanos()) else {
                *invalid = true;
                return;
            };
            Self::add_counter(total, Some(amount), invalid);
        }
    }

    #[expect(
        clippy::print_stderr,
        reason = "bounded opt-in causal marker is replayed against the scale artifact"
    )]
    fn emit(&self, result: &LiveBm25Statistics) {
        let Some(started) = self.started else {
            return;
        };
        if self.invalid {
            eprintln!("QI_CAUSAL_V1 kind=bm25_live_build ok=0 reason=counter_overflow");
            return;
        }
        let Ok(retained_estimate_bytes) = result.heap_bytes_estimate() else {
            eprintln!("QI_CAUSAL_V1 kind=bm25_live_build ok=0 reason=retained_estimate_failed");
            return;
        };
        let correction_keys = result.segments.iter().try_fold(0_u64, |count, segment| {
            let keys = u64::try_from(segment.dead_df.len())
                .map_err(|error| resource(&format!("correction key count width: {error}")))?;
            count
                .checked_add(keys)
                .ok_or_else(|| resource("correction key count overflow"))
        });
        let (Ok(elapsed_ns), Ok(correction_keys), Ok(segment_fanout)) = (
            u64::try_from(started.elapsed().as_nanos()),
            correction_keys,
            u64::try_from(result.segments.len()),
        ) else {
            eprintln!("QI_CAUSAL_V1 kind=bm25_live_build ok=0 reason=counter_overflow");
            return;
        };
        eprintln!(
            "QI_CAUSAL_V1 kind=bm25_live_build ok=1 elapsed_ns={elapsed_ns} reused_segments={} changed_segments={} new_segments={} mask_docs={} new_census_docs={} newly_dead_docs={} logical_census_bytes={} changed_ns={} new_ns={} death_ns={} correction_keys={correction_keys} segment_fanout={segment_fanout} retained_valid=1 retained_estimate_bytes={retained_estimate_bytes}",
            self.reused_segments,
            self.changed_segments,
            self.new_segments,
            self.mask_docs,
            self.new_census_docs,
            self.newly_dead_docs,
            self.logical_census_bytes,
            self.changed_ns,
            self.new_ns,
            self.death_ns,
        );
    }
}

fn corrupt(dir: &Path, reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!(
            "lexical: generation {} does not match its manifest: {FILE_NAME}: {reason}",
            dir.display()
        ),
    }
}

fn segment_identity(reader: &SegmentReader) -> SegmentIdentity {
    (
        tantivy::SegmentReader::segment_id(reader).to_string(),
        reader.max_doc(),
        reader.num_docs(),
        reader.delete_opstamp(),
    )
}

fn add_count(total: &mut u64, count: u64) -> Result<(), CoreError> {
    *total = total
        .checked_add(count)
        .ok_or_else(|| CoreError::Storage("lexical: live BM25 statistic overflow".into()))?;
    Ok(())
}

fn resource(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical: live BM25 statistic capacity: {reason}"))
}

fn charged_rows(segment: &SegmentStatistics) -> Result<usize, CoreError> {
    let keys = segment
        .dead_df
        .keys()
        .try_fold(0_usize, |total, (_, term)| {
            total.checked_add(term.capacity())?.checked_add(96)
        })
        .ok_or_else(|| resource("term bytes overflow"))?;
    let field_bytes = segment
        .field_tokens
        .len()
        .checked_mul(32)
        .ok_or_else(|| resource("field row bytes overflow"))?;
    let identity_bytes = segment
        .identity
        .0
        .capacity()
        .checked_add(96)
        .ok_or_else(|| resource("segment identity bytes overflow"))?;
    keys.checked_add(field_bytes)
        .and_then(|bytes| bytes.checked_add(identity_bytes))
        .ok_or_else(|| resource("row bytes overflow"))
}

fn admit_segment(segment: &SegmentStatistics) -> Result<(), CoreError> {
    if charged_rows(segment)? > MAX_BYTES {
        return Err(resource("segment statistics exceed control-file limit"));
    }
    Ok(())
}

fn push_segment(
    segments: &mut Vec<SegmentStatistics>,
    admitted: &mut usize,
    segment: SegmentStatistics,
) -> Result<(), CoreError> {
    admit_segment(&segment)?;
    *admitted = (*admitted)
        .checked_add(charged_rows(&segment)?)
        .ok_or_else(|| resource("generation residency overflow"))?;
    // A producer still has to encode up to the full control-file ceiling.
    if *admitted > MAX_RESIDENT - MAX_BYTES {
        return Err(resource(
            "generation statistics exceed resident table limit",
        ));
    }
    segments.push(segment);
    Ok(())
}

fn apply_census(
    statistics: &mut SegmentStatistics,
    charged: &mut usize,
    census: &DocCensus,
    alive: bool,
) -> Result<(), CoreError> {
    for (&field, (tokens, terms)) in &census.fields {
        if alive {
            add_count(statistics.field_tokens.entry(field).or_insert(0), *tokens)?;
        } else {
            for term in terms {
                let key = (field, term.clone());
                match statistics.dead_df.entry(key) {
                    Entry::Vacant(entry) => {
                        let next = term
                            .len()
                            .checked_add(96)
                            .and_then(|bytes| (*charged).checked_add(bytes))
                            .ok_or_else(|| resource("correction bytes overflow"))?;
                        if next > MAX_BYTES {
                            return Err(resource("corrections exceed control-file limit"));
                        }
                        *charged = next;
                        let _inserted = entry.insert(1);
                    }
                    Entry::Occupied(mut entry) => add_count(entry.get_mut(), 1)?,
                }
            }
        }
    }
    Ok(())
}

fn subtract_census(
    statistics: &mut SegmentStatistics,
    charged: &mut usize,
    census: &DocCensus,
) -> Result<(), CoreError> {
    for (&field, (tokens, terms)) in &census.fields {
        let value = statistics
            .field_tokens
            .get_mut(&field)
            .ok_or_else(|| resource("carried field statistic missing"))?;
        *value = value
            .checked_sub(*tokens)
            .ok_or_else(|| resource("carried field token count underflows"))?;
        for term in terms {
            let key = (field, term.clone());
            match statistics.dead_df.entry(key) {
                Entry::Vacant(entry) => {
                    let next = term
                        .len()
                        .checked_add(96)
                        .and_then(|bytes| (*charged).checked_add(bytes))
                        .ok_or_else(|| resource("correction bytes overflow"))?;
                    if next > MAX_BYTES {
                        return Err(resource("corrections exceed control-file limit"));
                    }
                    *charged = next;
                    let _inserted = entry.insert(1);
                }
                Entry::Occupied(mut entry) => add_count(entry.get_mut(), 1)?,
            }
        }
    }
    Ok(())
}

fn census_at(
    searcher: &Searcher,
    index: &Index,
    fields: &SchemaFields,
    ordinal: usize,
    doc_id: u32,
) -> Result<(DocCensus, usize), CoreError> {
    let ordinal = u32::try_from(ordinal)
        .map_err(|error| resource(&format!("segment ordinal overflow: {error}")))?;
    let document: TantivyDocument = searcher
        .doc(DocAddress::new(ordinal, doc_id))
        .map_err(|error| CoreError::Storage(format!("lexical: read document census: {error}")))?;
    let census = DocCensus::from_stored(index, fields, &document)?;
    // The decoded document is already held by this caller. No second store read.
    let logical_bytes = document
        .get_first(fields.live_bm25_doc_census)
        .and_then(|value| value.as_bytes())
        .map(<[u8]>::len)
        .ok_or_else(|| {
            CoreError::Storage("lexical: validated BM25 census disappeared during read".into())
        })?;
    Ok((census, logical_bytes))
}

fn immutable_components_match(
    target_meta: &tantivy::SegmentMeta,
    base_meta: &tantivy::SegmentMeta,
    target: &BTreeMap<&str, &SealedArtifactCommitmentV1>,
    base: &BTreeMap<&str, &SealedArtifactCommitmentV1>,
) -> bool {
    for component in SegmentComponent::iterator() {
        if matches!(
            component,
            SegmentComponent::TempStore | SegmentComponent::Delete
        ) {
            continue;
        }
        let target_name = target_meta.relative_path(*component);
        let base_name = base_meta.relative_path(*component);
        let (Some(target_name), Some(base_name)) = (target_name.to_str(), base_name.to_str())
        else {
            return false;
        };
        let (Some(target_file), Some(base_file)) = (target.get(target_name), base.get(base_name))
        else {
            return false;
        };
        if target_file.bytes != base_file.bytes || target_file.sha256 != base_file.sha256 {
            return false;
        }
    }
    true
}

impl LiveBm25Statistics {
    /// Build against the finalized commit. A sealed base's sidecar is read
    /// under its manifest and segment identity before any row is reused.
    pub(crate) fn build(
        index: &Index,
        fields: &SchemaFields,
        meta_digest: [u8; 32],
        generation_dir: &Path,
        base: Option<&BaseSnapshot>,
        index_segments: &[SealedArtifactCommitmentV1],
    ) -> Result<Self, CoreError> {
        let mut observation = BuildObservation::new();
        let reader = index.reader().map_err(|error| {
            CoreError::Storage(format!("lexical: open BM25 seal reader: {error}"))
        })?;
        let searcher = reader.searcher();
        let schema = index.schema();
        let indexed: Vec<_> = schema
            .fields()
            .filter(|(_, entry)| entry.field_type().is_indexed())
            .map(|(field, _)| field.field_id())
            .collect();
        let current_meta: BTreeMap<_, _> = index
            .searchable_segment_metas()
            .map_err(|error| CoreError::Storage(format!("lexical: list BM25 segments: {error}")))?
            .into_iter()
            .map(|meta| (meta.id().to_string(), meta))
            .collect();
        let current_files: BTreeMap<_, _> = index_segments
            .iter()
            .map(|row| (row.name.as_str(), row))
            .collect();
        let base_reader = base
            .as_ref()
            .map(|base| base.index.reader())
            .transpose()
            .map_err(|error| {
                CoreError::Storage(format!("lexical: open BM25 base reader: {error}"))
            })?;
        let base_searcher = base_reader.as_ref().map(tantivy::IndexReader::searcher);
        let base_meta: BTreeMap<_, _> = base
            .as_ref()
            .map(|base| {
                base.index.searchable_segment_metas().map(|metas| {
                    metas
                        .into_iter()
                        .map(|meta| (meta.id().to_string(), meta))
                        .collect()
                })
            })
            .transpose()
            .map_err(|error| {
                CoreError::Storage(format!("lexical: list BM25 base segments: {error}"))
            })?
            .unwrap_or_default();
        let base_files: BTreeMap<_, _> = base
            .as_ref()
            .map(|base| {
                base.index_segments
                    .iter()
                    .map(|row| (row.name.as_str(), row))
                    .collect()
            })
            .unwrap_or_default();
        let base_rows: BTreeMap<_, _> = base
            .as_ref()
            .map(|base| {
                base.statistics
                    .segments
                    .iter()
                    .map(|row| (row.identity.0.as_str(), row))
                    .collect()
            })
            .unwrap_or_default();
        let base_ordinals: BTreeMap<_, _> = base_searcher
            .as_ref()
            .map(|searcher| {
                searcher
                    .segment_readers()
                    .iter()
                    .enumerate()
                    .map(|(ordinal, segment)| {
                        (
                            tantivy::SegmentReader::segment_id(segment).to_string(),
                            ordinal,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(searcher.segment_readers().len())
            .map_err(|error| resource(&format!("segment allocation: {error}")))?;
        let mut admitted = 0_usize;
        for (ordinal, segment) in searcher.segment_readers().iter().enumerate() {
            let identity = segment_identity(segment);
            if let (
                Some(base),
                Some(base_searcher),
                Some(&old_ordinal),
                Some(old_row),
                Some(target_meta),
                Some(old_meta),
            ) = (
                base.as_ref(),
                base_searcher.as_ref(),
                base_ordinals.get(&identity.0),
                base_rows.get(identity.0.as_str()),
                current_meta.get(&identity.0),
                base_meta.get(&identity.0),
            ) {
                let old_segment = base_searcher
                    .segment_readers()
                    .get(old_ordinal)
                    .ok_or_else(|| corrupt(generation_dir, "carried segment ordinal is missing"))?;
                if identity.1 != old_row.identity.1 || identity.1 != old_segment.max_doc() {
                    return Err(corrupt(generation_dir, "carried segment max_doc changed"));
                }
                if !immutable_components_match(target_meta, old_meta, &current_files, &base_files) {
                    return Err(corrupt(
                        generation_dir,
                        "carried segment immutable components changed",
                    ));
                }
                let mut statistics = (**old_row).clone();
                let mut segment_charged = charged_rows(&statistics)?;
                let target_delete = target_meta.relative_path(SegmentComponent::Delete);
                let base_delete = old_meta.relative_path(SegmentComponent::Delete);
                let target_delete = target_delete
                    .to_str()
                    .and_then(|name| current_files.get(name));
                let base_delete = base_delete.to_str().and_then(|name| base_files.get(name));
                let same_delete = match (target_delete, base_delete) {
                    (None, None) => true,
                    (Some(target), Some(base)) => {
                        target.bytes == base.bytes && target.sha256 == base.sha256
                    }
                    _ => false,
                };
                if identity == old_row.identity {
                    if !same_delete {
                        return Err(corrupt(
                            generation_dir,
                            "unchanged deletion identity has different bitmap bytes",
                        ));
                    }
                    push_segment(&mut segments, &mut admitted, statistics)?;
                    if observation.started.is_some() {
                        BuildObservation::add_counter(
                            &mut observation.reused_segments,
                            Some(1),
                            &mut observation.invalid,
                        );
                    }
                    continue;
                }
                if same_delete {
                    return Err(corrupt(
                        generation_dir,
                        "changed deletion identity retained bitmap bytes",
                    ));
                }
                let changed_started = observation.started.map(|_| Instant::now());
                let mut new_deaths = 0_u64;
                for doc_id in 0..segment.max_doc() {
                    let was_dead = old_segment.is_deleted(doc_id);
                    let is_dead = segment.is_deleted(doc_id);
                    if was_dead && !is_dead {
                        return Err(corrupt(
                            generation_dir,
                            "carried segment resurrected a document",
                        ));
                    }
                    if !was_dead && is_dead {
                        let death_started = observation.started.map(|_| Instant::now());
                        let (census, logical_bytes) =
                            census_at(base_searcher, &base.index, fields, old_ordinal, doc_id)?;
                        subtract_census(&mut statistics, &mut segment_charged, &census)?;
                        add_count(&mut new_deaths, 1)?;
                        if observation.started.is_some() {
                            BuildObservation::add_counter(
                                &mut observation.newly_dead_docs,
                                Some(1),
                                &mut observation.invalid,
                            );
                            observation.add_bytes(logical_bytes);
                            BuildObservation::add_elapsed(
                                &mut observation.death_ns,
                                death_started,
                                &mut observation.invalid,
                            );
                        }
                    }
                }
                let expected_old_live = u64::from(segment.num_docs())
                    .checked_add(new_deaths)
                    .ok_or_else(|| corrupt(generation_dir, "carried deletion count overflows"))?;
                if u64::from(old_segment.num_docs()) != expected_old_live {
                    return Err(corrupt(
                        generation_dir,
                        "carried segment deletion count differs",
                    ));
                }
                statistics.identity = identity;
                push_segment(&mut segments, &mut admitted, statistics)?;
                if observation.started.is_some() {
                    BuildObservation::add_counter(
                        &mut observation.changed_segments,
                        Some(1),
                        &mut observation.invalid,
                    );
                    BuildObservation::add_counter(
                        &mut observation.mask_docs,
                        Some(u64::from(segment.max_doc())),
                        &mut observation.invalid,
                    );
                    BuildObservation::add_elapsed(
                        &mut observation.changed_ns,
                        changed_started,
                        &mut observation.invalid,
                    );
                }
                continue;
            }
            let new_started = observation.started.map(|_| Instant::now());
            let mut statistics = SegmentStatistics {
                identity,
                field_tokens: indexed.iter().map(|field| (*field, 0)).collect(),
                dead_df: BTreeMap::new(),
            };
            let mut segment_charged = charged_rows(&statistics)?;
            for doc_id in 0..segment.max_doc() {
                let (census, logical_bytes) = census_at(&searcher, index, fields, ordinal, doc_id)?;
                apply_census(
                    &mut statistics,
                    &mut segment_charged,
                    &census,
                    !segment.is_deleted(doc_id),
                )?;
                if observation.started.is_some() {
                    BuildObservation::add_counter(
                        &mut observation.new_census_docs,
                        Some(1),
                        &mut observation.invalid,
                    );
                    observation.add_bytes(logical_bytes);
                }
            }
            push_segment(&mut segments, &mut admitted, statistics)?;
            if observation.started.is_some() {
                BuildObservation::add_counter(
                    &mut observation.new_segments,
                    Some(1),
                    &mut observation.invalid,
                );
                BuildObservation::add_elapsed(
                    &mut observation.new_ns,
                    new_started,
                    &mut observation.invalid,
                );
            }
        }
        let result = Self::from_segments(meta_digest, searcher.num_docs(), segments, &indexed, 0)?;
        observation.emit(&result);
        Ok(result)
    }

    fn from_segments(
        index_meta_digest: [u8; 32],
        live_docs: u64,
        segments: Vec<SegmentStatistics>,
        indexed: &[u32],
        encoded_bytes: usize,
    ) -> Result<Self, CoreError> {
        let mut field_tokens: BTreeMap<u32, u64> =
            indexed.iter().map(|field| (*field, 0)).collect();
        let mut resident = encoded_bytes;
        for segment in &segments {
            resident = resident
                .checked_add(charged_rows(segment)?)
                .ok_or_else(|| resource("segment residency overflow"))?;
            if resident > MAX_RESIDENT {
                return Err(resource(
                    "segment residency exceeds the existing table limit",
                ));
            }
            if segment.field_tokens.len() != indexed.len() {
                return Err(CoreError::Storage(
                    "lexical: indexed BM25 field statistics are incomplete".into(),
                ));
            }
            for (&field, &tokens) in &segment.field_tokens {
                let total = field_tokens.get_mut(&field).ok_or_else(|| {
                    CoreError::Storage("lexical: unknown indexed BM25 field statistic".into())
                })?;
                add_count(total, tokens)?;
            }
        }
        resident = resident
            .checked_add(field_tokens.len().saturating_mul(32))
            .ok_or_else(|| resource("aggregate field residency overflow"))?;
        if resident > MAX_RESIDENT {
            return Err(resource("statistics exceed decoded resident limit"));
        }
        Ok(Self {
            index_meta_digest,
            live_docs,
            segments,
            field_tokens,
            encoded_bytes,
        })
    }

    pub(crate) fn heap_bytes_estimate(&self) -> Result<u64, CoreError> {
        // The encoded file is excluded from mapped-file accounting at open.
        // Segment maps retain each term once; only field totals are aggregated.
        let segment_bytes = self.segments.iter().try_fold(0_usize, |bytes, segment| {
            bytes.checked_add(charged_rows(segment)?).ok_or_else(|| {
                CoreError::Storage("lexical: BM25 segment residency overflow".into())
            })
        })?;
        let estimate = segment_bytes
            .checked_add(self.encoded_bytes)
            .and_then(|bytes| {
                self.field_tokens
                    .len()
                    .checked_mul(32)
                    .and_then(|fields| bytes.checked_add(fields))
            })
            .ok_or_else(|| CoreError::Storage("lexical: BM25 residency overflow".into()))?;
        u64::try_from(estimate)
            .map_err(|error| CoreError::Storage(format!("lexical: BM25 residency width: {error}")))
    }
}

pub(crate) struct LiveBm25Provider<'a> {
    pub(crate) statistics: &'a LiveBm25Statistics,
    pub(crate) searcher: &'a Searcher,
}

impl Bm25StatisticsProvider for LiveBm25Provider<'_> {
    fn total_num_tokens(&self, field: Field) -> tantivy::Result<u64> {
        self.statistics
            .field_tokens
            .get(&field.field_id())
            .copied()
            .ok_or_else(|| {
                tantivy::TantivyError::InvalidArgument("missing committed live BM25 field".into())
            })
    }

    fn total_num_docs(&self) -> tantivy::Result<u64> {
        Ok(self.statistics.live_docs)
    }

    fn doc_freq(&self, term: &Term) -> tantivy::Result<u64> {
        let raw = self.searcher.doc_freq(term)?;
        let key = (
            term.field().field_id(),
            term.serialized_value_bytes().to_vec(),
        );
        let mut dead = 0_u64;
        for segment in &self.statistics.segments {
            if let Some(count) = segment.dead_df.get(&key) {
                dead = dead.checked_add(*count).ok_or_else(|| {
                    tantivy::TantivyError::InvalidArgument("committed BM25 df overflow".into())
                })?;
            }
        }
        raw.checked_sub(dead).ok_or_else(|| {
            tantivy::TantivyError::InvalidArgument("committed BM25 df exceeds raw df".into())
        })
    }
}

#[cfg(test)]
mod tests;
