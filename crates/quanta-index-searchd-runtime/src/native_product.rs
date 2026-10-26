//! Explicit native daemon composition for the product process entry.
//! The legacy `serve` command and its state format are separate.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read as _,
    os::unix::fs::DirBuilderExt as _,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use quanta_index_contract::EmbeddingNormalization;
use quanta_index_contract::index::{
    Digest, IndexKind, ProducerId, ProjectionId, SchemaId, StorageFormatId, TenantId,
};
use quanta_index_contract::native_protocol::NativeRepoMapQuery;
use quanta_index_core::index::{
    AttemptContext, CoverageBinding, IndexError, IndexQueryPort, InstanceSpec, MapRoot, ObjectRef,
    OwnedWorkPolicy, OwnerInputBinding, OwnerKey, OwnerMemberBinding, QueryOutcome,
    QueryReadContext, WorkLimits, WorkspaceKey,
    effort::{
        DeclaredCostEstimator, HysteresisConfig, HysteresisPolicy, PressureThresholds, ResourceNeed,
    },
    ports::{IndexFuture, IndexOwnerBuildPort},
    validate_read_ticket_lease,
};
use quanta_index_core::{
    CoreError, L2UnitEmbeddingProvider, RequestBudgetV1, SemanticAdmissionEngine,
    SemanticEgressGrantV1, SemanticEgressPolicyV1, SemanticInputClass, TextEmbeddingProvider,
};
use quanta_index_embed::{OpenAiEmbeddingProvider, OpenAiProviderConfig, ProviderAttemptPool};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, QueryAdapterRegistry};
use quanta_index_lexical::native::{
    Bm25Policy, NativeLiteralIndex, NativeLiteralReader, NativeTextIndex, NativeTextReader,
    PlannedTextRead, TextReadResult, TextRecipe,
};
use quanta_index_repomap::native::graph::{GraphPage, GraphQuery, NativeGraph, NativeGraphReader};
use quanta_index_repomap::native::{
    NativeFileMap, NativePathReader, NativeRepoMapRankPage, NativeRepoMapRankReader,
};
use quanta_index_semantic::native::{
    EmbeddingProvider, EmbeddingRecipe, NativeSemantic, NativeSemanticReadConfig,
    NativeSemanticReader, SearchResult, SemanticCandidate, SemanticReadRequest,
    TextEmbeddingBridge, Utf8ChunkPolicy,
};
use rustix::fs::{Mode, OFlags, open};
use serde::Deserialize;
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::{Handle as SignalHandle, Signals},
};

use crate::native::{
    FixedPumpPolicy, NativeIndexService, NativePhysicalProfile, NativePumpConfig, NativePumpScope,
    NativeReclaimConfig, NativeResourcePlan, NativeRetentionConfig, NativeRetentionScope,
    NativeServiceCapabilities, NativeServiceResources, validate_automatic_reclaim,
};
use crate::native_effort::NativeEffortConfiguration;

#[path = "native_boot.rs"]
mod boot;
#[path = "native_product_diagnostic.rs"]
mod diagnostic;

use quanta_index_searchd::cli::{NativeCommandModeV1, NativeStateCommandV1};
use sha2::{Digest as _, Sha256};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const ACCEPT_IDLE: Duration = Duration::from_millis(50);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeServeConfigV1 {
    version: u32,
    state_root: PathBuf,
    socket_path: PathBuf,
    work_limits: WorkLimitsConfig,
    resources: ResourcesConfig,
    literal: DescriptorConfig,
    file_map: DescriptorConfig,
    text: TextConfig,
    #[serde(default)]
    maintenance: Option<MaintenanceConfig>,
    #[serde(default)]
    retention: Option<RetentionConfig>,
    #[serde(default)]
    read_sessions: Option<ReadSessionsConfig>,
    #[serde(default)]
    owned_work: Option<OwnedWorkConfig>,
    #[serde(default)]
    semantic: Option<SemanticConfig>,
    #[serde(default)]
    graph: Option<GraphConfig>,
    #[serde(default)]
    effort: Option<EffortConfig>,
    #[serde(default)]
    overlay: Option<OverlayConfig>,
    #[serde(default)]
    stub_ast: Option<StubAstConfig>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkLimitsConfig {
    reads: u64,
    writes: u64,
    bytes: u64,
    mutations: u64,
}

impl WorkLimitsConfig {
    fn validate(self) -> Result<WorkLimits> {
        if self.reads == 0 || self.writes == 0 || self.bytes == 0 || self.mutations == 0 {
            bail!("native work limits must be positive");
        }
        Ok(WorkLimits {
            reads: self.reads,
            writes: self.writes,
            bytes: self.bytes,
            mutations: self.mutations,
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct NeedConfig {
    cpu_units: u64,
    io_bytes: u64,
    memory_bytes: u64,
    provider_units: u64,
}

impl From<NeedConfig> for ResourceNeed {
    fn from(value: NeedConfig) -> Self {
        Self {
            cpu_units: value.cpu_units,
            io_bytes: value.io_bytes,
            memory_bytes: value.memory_bytes,
            provider_units: value.provider_units,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionConfig {
    capacity: NeedConfig,
    control: NeedConfig,
    interactive: NeedConfig,
    core: NeedConfig,
    enrichment: NeedConfig,
}

impl ExecutionConfig {
    fn into_plan(self) -> NativeResourcePlan {
        NativeResourcePlan {
            capacity: self.capacity.into(),
            control: self.control.into(),
            interactive: self.interactive.into(),
            core: self.core.into(),
            enrichment: self.enrichment.into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourcesConfig {
    physical: NativePhysicalProfile,
    scratch_reserved_bytes: u64,
    read_ticket_lease_ms: u64,
    execution: ExecutionConfig,
}

impl ResourcesConfig {
    fn validate(self) -> Result<NativeServiceResources> {
        self.physical.validate()?;
        if self.scratch_reserved_bytes == 0 {
            bail!("native scratch reservation must be positive");
        }
        let read_ticket_lease = Duration::from_millis(self.read_ticket_lease_ms);
        let _lease_millis = validate_read_ticket_lease(read_ticket_lease)?;
        let execution = self.execution.into_plan();
        execution.validate()?;
        Ok(NativeServiceResources {
            physical: self.physical,
            scratch_reserved_bytes: self.scratch_reserved_bytes,
            read_ticket_lease,
            execution,
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescriptorConfig {
    kind: [u8; 16],
    schema: [u8; 16],
    format: [u8; 16],
}

impl DescriptorConfig {
    fn descriptor(self) -> AdapterDescriptor {
        AdapterDescriptor {
            kind: IndexKind(self.kind),
            schema: SchemaId(self.schema),
            format: StorageFormatId(self.format),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bm25Config {
    k1: f64,
    b: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextConfig {
    descriptor: DescriptorConfig,
    admission_policy: [u8; 32],
    bm25: Bm25Config,
    read_need: NeedConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphConfig {
    descriptor: DescriptorConfig,
    read_need: NeedConfig,
}

struct ValidatedGraphConfig {
    descriptor: AdapterDescriptor,
    read_need: ResourceNeed,
}

impl GraphConfig {
    fn validate(self, resources: NativeServiceResources) -> Result<ValidatedGraphConfig> {
        let read_need: ResourceNeed = self.read_need.into();
        let _reader = NativeGraphReader::new(read_need)?;
        if !read_need.fits(resources.execution.interactive) {
            bail!("native graph reader exceeds Interactive envelope");
        }
        Ok(ValidatedGraphConfig {
            descriptor: self.descriptor.descriptor(),
            read_need,
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayConfig {
    text_instance: [u8; 16],
    sessions: usize,
    documents: usize,
    document_bytes: usize,
    total_bytes: usize,
}

impl OverlayConfig {
    fn validate(self) -> Result<Self> {
        if self.text_instance == [0; 16] {
            bail!("Native overlay requires a nonzero searchable text instance");
        }
        let _validated = crate::native_overlay::OverlayLimits {
            sessions: self.sessions,
            documents: self.documents,
            document_bytes: self.document_bytes,
            total_bytes: self.total_bytes,
        }
        .validate()?;
        Ok(self)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StubAstConfig {
    policies: Vec<StubGrammarPolicyConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StubGrammarPolicyConfig {
    language: String,
    grammar_engine: String,
    grammar_version: String,
    grammar_hash: Option<[u8; 8]>,
    grammar_policy: [u8; 32],
}

impl StubAstConfig {
    fn validate(self) -> Result<Vec<crate::native::StubGrammarPolicy>> {
        if self.policies.is_empty() || self.policies.len() > 16 {
            bail!("Native AST grammar policy count must be within 1..=16");
        }
        let policies = self
            .policies
            .into_iter()
            .map(|policy| crate::native::StubGrammarPolicy {
                language: policy.language,
                grammar_engine: policy.grammar_engine,
                grammar_version: policy.grammar_version,
                grammar_hash: policy.grammar_hash,
                grammar_policy: Digest(policy.grammar_policy),
            })
            .collect::<Vec<_>>();
        crate::native_stub_certificate_projection_v1::validate_stub_grammar_policies(&policies)?;
        Ok(policies)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticConfig {
    scopes: Vec<ScopeConfig>,
    descriptor: DescriptorConfig,
    provider: SemanticProviderConfig,
    max_source_bytes: u64,
    #[serde(default)]
    input: SemanticInputConfig,
    read: SemanticReadPolicyConfig,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SemanticInputConfig {
    #[default]
    SourceUtf8,
    FileLocalChunks {
        max_utf8_bytes: u32,
        max_vectors: u32,
    },
    CertifiedProjectionChunks {
        projection: [u8; 16],
        producer: [u8; 16],
        schema: [u8; 16],
        max_vectors: u32,
    },
}

impl SemanticInputConfig {
    fn max_vectors(self) -> u32 {
        match self {
            Self::SourceUtf8 => 1,
            Self::FileLocalChunks { max_vectors, .. }
            | Self::CertifiedProjectionChunks { max_vectors, .. } => max_vectors,
        }
    }

    fn validate(self, dimension: usize) -> Result<()> {
        if let Self::FileLocalChunks { max_utf8_bytes, .. } = self {
            Utf8ChunkPolicy { max_utf8_bytes }.validate()?;
        }
        let vectors = self.max_vectors();
        if vectors == 0
            || vectors > 4096
            || dimension
                .checked_mul(usize::try_from(vectors)?)
                .is_none_or(|cells| cells > 200_000)
        {
            bail!("native Semantic input exceeds vector and coordinate limits");
        }
        Ok(())
    }

    fn bridge<P: TextEmbeddingProvider>(self, provider: P) -> Result<TextEmbeddingBridge<P>> {
        match self {
            Self::SourceUtf8 => Ok(TextEmbeddingBridge::new(provider)?),
            Self::FileLocalChunks { max_utf8_bytes, .. } => {
                Ok(TextEmbeddingBridge::with_utf8_chunks(
                    provider,
                    Utf8ChunkPolicy { max_utf8_bytes },
                )?)
            }
            Self::CertifiedProjectionChunks { .. } => {
                Ok(TextEmbeddingBridge::with_certified_chunks(provider)?)
            }
        }
    }

    fn recipe<P: TextEmbeddingProvider>(
        self,
        bridge: &TextEmbeddingBridge<P>,
        max_source_bytes: u64,
    ) -> Result<EmbeddingRecipe> {
        match self {
            Self::SourceUtf8 => Ok(bridge.recipe(max_source_bytes)?),
            Self::FileLocalChunks { max_vectors, .. } => {
                Ok(bridge.chunk_recipe(max_source_bytes, max_vectors)?)
            }
            Self::CertifiedProjectionChunks {
                projection,
                producer,
                schema,
                max_vectors,
            } => Ok(bridge.certified_chunk_recipe(
                ProjectionId(projection),
                ProducerId(producer),
                SchemaId(schema),
                max_source_bytes,
                max_vectors,
            )?),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticReadPolicyConfig {
    page_size: u32,
    max_owners: u32,
    max_query_bytes: usize,
    need: NeedConfig,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SemanticProviderConfig {
    // Local weights need a startup residency grant on the same native ledger
    // before the decoder allocates them; only the outbound provider can be
    // composed by this process entry until that owner exists.
    OpenAi {
        model: String,
        model_revision: String,
        dimension: usize,
        /// Environment variable name; API keys do not live in config files.
        api_key_env: String,
        base_url: String,
        max_batch: usize,
        max_estimated_tokens_per_request: usize,
        max_retries: u32,
        timeout_ms: u64,
        max_response_bytes: u64,
        concurrency: usize,
        egress: SemanticEgressConfig,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticEgressConfig {
    tenant_id: TenantId,
    endpoint: String,
    region: String,
    retention: String,
    profile: String,
    source_content_consent: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PressureThresholdsConfig {
    interactive_low_bps: u16,
    interactive_high_bps: u16,
    backlog_low: u32,
    backlog_high: u32,
    lag_low_ms: u64,
    lag_high_ms: u64,
    memory_clear_bytes: u64,
    memory_high_pressure_bytes: u64,
}

impl From<PressureThresholdsConfig> for PressureThresholds {
    fn from(value: PressureThresholdsConfig) -> Self {
        Self {
            interactive_low_bps: value.interactive_low_bps,
            interactive_high_bps: value.interactive_high_bps,
            backlog_low: value.backlog_low,
            backlog_high: value.backlog_high,
            lag_low_ms: value.lag_low_ms,
            lag_high_ms: value.lag_high_ms,
            memory_clear_bytes: value.memory_clear_bytes,
            memory_high_pressure_bytes: value.memory_high_pressure_bytes,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffortConfig {
    thresholds: PressureThresholdsConfig,
    stable_idle_ms: u64,
    clear_dwell_ms: u64,
    minimum_residence_ms: u64,
    worker_lag_max_age_ms: u64,
    sample_ttl_ms: u64,
    /// Select the foreground source for this daemon explicitly. The publisher
    /// must observe the intended UI; activity in other apps is not inferred.
    #[serde(default)]
    interaction_source: Option<ForegroundObservationSource>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ForegroundObservationSource {
    RegisteredForegroundPublisher,
}

#[derive(Clone, Copy)]
struct ValidatedEffortConfig {
    policy: HysteresisConfig,
    worker_lag_max_age: Duration,
    sample_ttl: Duration,
    interaction_source: Option<ForegroundObservationSource>,
}

impl EffortConfig {
    fn validate(self) -> Result<ValidatedEffortConfig> {
        let policy = HysteresisConfig {
            thresholds: self.thresholds.into(),
            stable_idle_ms: self.stable_idle_ms,
            clear_dwell_ms: self.clear_dwell_ms,
            minimum_residence_ms: self.minimum_residence_ms,
        };
        policy.validate()?;
        if !(1..=60_000).contains(&self.worker_lag_max_age_ms)
            || !(1..=60_000).contains(&self.sample_ttl_ms)
        {
            bail!("native Auto signal age and TTL must be 1..=60000 ms");
        }
        Ok(ValidatedEffortConfig {
            policy,
            worker_lag_max_age: Duration::from_millis(self.worker_lag_max_age_ms),
            sample_ttl: Duration::from_millis(self.sample_ttl_ms),
            interaction_source: self.interaction_source,
        })
    }
}

struct ValidatedSemanticConfig {
    scopes: Arc<BTreeSet<NativePumpScope>>,
    descriptor: AdapterDescriptor,
    provider: SemanticProviderConfig,
    egress_policy: SemanticEgressPolicyV1,
    max_source_bytes: u64,
    input: SemanticInputConfig,
    read: NativeSemanticReadConfig,
    provider_escaping_bytes: u64,
}

/// Conservative logical startup admission for a chunk owner, including the
/// selected payload, a decoded prior artifact, reconciliation, and a provider
/// batch.
///
/// This is an allocation contract, not a measured process RSS ceiling.
fn chunk_owner_memory_envelope(
    input: SemanticInputConfig,
    provider: &OpenAiProviderConfig,
    max_source_bytes: u64,
    dimension: usize,
) -> Result<(u64, u64)> {
    input.validate(dimension)?;
    if max_source_bytes == 0 || max_source_bytes > 16 * 1024 * 1024 {
        bail!("native Semantic source bound exceeds exact owner limits");
    }
    let count = u64::from(input.max_vectors());
    let remaining = count
        .checked_sub(1)
        .ok_or_else(|| anyhow!("native Semantic chunk count is empty"))?;
    let (one_peak, one_escape) = provider.single_input_memory_envelope(max_source_bytes)?;
    let (empty_peak, empty_escape) = provider.single_input_memory_envelope(0)?;
    // All rendered text bytes are inside the bounded selected payload. Count
    // a complete request frame and response ceiling for every member: this
    // dominates any batch partition with one funded provider attempt slot.
    let provider_peak = one_peak
        .checked_add(
            empty_peak
                .checked_mul(remaining)
                .ok_or_else(|| anyhow!("native Semantic provider batch envelope overflows"))?,
        )
        .ok_or_else(|| anyhow!("native Semantic provider batch envelope overflows"))?;
    let provider_escape = one_escape
        .checked_add(
            empty_escape
                .checked_mul(remaining)
                .ok_or_else(|| anyhow!("native Semantic provider escape overflows"))?,
        )
        .ok_or_else(|| anyhow!("native Semantic provider escape overflows"))?;
    Ok(quanta_index_semantic::native::chunk_owner_memory_envelope(
        u32::try_from(dimension)
            .map_err(|_error| anyhow!("native Semantic dimension overflows"))?,
        input.max_vectors(),
        max_source_bytes,
        provider_peak,
        provider_escape,
    )?)
}

impl SemanticConfig {
    fn validate(self, resources: NativeServiceResources) -> Result<ValidatedSemanticConfig> {
        let declared_scope_count = self.scopes.len();
        if declared_scope_count == 0 || declared_scope_count > 1024 {
            bail!("native Semantic needs 1..=1024 explicit scopes");
        }
        let scopes = self
            .scopes
            .into_iter()
            .map(|scope| NativePumpScope {
                workspace: scope.workspace,
                epoch: scope.epoch,
            })
            .collect::<BTreeSet<_>>();
        if scopes.len() != declared_scope_count {
            bail!("native Semantic scope is duplicated");
        }
        if scopes
            .iter()
            .any(|scope| i64::try_from(scope.epoch).is_err())
        {
            bail!("native Semantic scope epoch exceeds catalog range");
        }
        let tenants = scopes
            .iter()
            .map(|scope| scope.workspace.tenant)
            .collect::<BTreeSet<_>>();
        if tenants.len() != 1 {
            bail!("native Semantic egress grant cannot span workspace tenants");
        }
        let read = NativeSemanticReadConfig {
            page_size: self.read.page_size,
            max_owners: self.read.max_owners,
            max_query_bytes: self.read.max_query_bytes,
            need: self.read.need.into(),
        };
        read.validate()?;
        if !read.need.fits(resources.execution.interactive) {
            bail!("native semantic reader reservation exceeds Interactive envelope");
        }
        if self.max_source_bytes == 0 || self.max_source_bytes > 16 * 1024 * 1024 {
            bail!("native semantic source bound exceeds exact owner limits");
        }
        let (egress_policy, provider_escaping_bytes) = match &self.provider {
            SemanticProviderConfig::OpenAi {
                model,
                model_revision,
                dimension,
                api_key_env,
                base_url,
                max_batch,
                max_estimated_tokens_per_request,
                timeout_ms,
                max_response_bytes,
                concurrency,
                egress,
                ..
            } => {
                if scopes
                    .iter()
                    .any(|scope| scope.workspace.tenant != egress.tenant_id)
                {
                    bail!("native Semantic egress tenant differs from authorized scopes");
                }
                if model.trim().is_empty()
                    || model_revision.trim().is_empty()
                    || !(1..=4096).contains(dimension)
                    || *max_batch == 0
                    || *max_estimated_tokens_per_request == 0
                    || *timeout_ms == 0
                    || *max_response_bytes == 0
                    || !(1..=64).contains(concurrency)
                    || !valid_environment_name(api_key_env)
                {
                    bail!("invalid native OpenAI provider configuration");
                }
                self.input.validate(*dimension)?;
                if *max_response_bytes > read.need.memory_bytes
                    || *max_response_bytes > resources.execution.enrichment.memory_bytes
                {
                    bail!(
                        "native OpenAI response bound exceeds reader or Enrichment memory reservation"
                    );
                }
                let envelope = OpenAiProviderConfig::new(
                    String::new(),
                    model.clone(),
                    model_revision.clone(),
                    *dimension,
                )
                .with_base_url(base_url.clone())
                .with_max_response_bytes(*max_response_bytes);
                let (reader_peak, reader_escape) = envelope.single_input_memory_envelope(
                    u64::try_from(read.max_query_bytes).map_err(|_error| {
                        anyhow!("native query bound exceeds provider envelope")
                    })?,
                )?;
                let (owner_peak, owner_escape) = match self.input {
                    SemanticInputConfig::SourceUtf8 => {
                        let (peak, escape) =
                            envelope.single_input_memory_envelope(self.max_source_bytes)?;
                        // The maximal one-member artifact shape also dominates
                        // the unchunked shape; include current bytes and prior
                        // decode backing instead of only the provider output.
                        quanta_index_semantic::native::chunk_owner_memory_envelope(
                            u32::try_from(*dimension)
                                .map_err(|_error| anyhow!("native Semantic dimension overflows"))?,
                            1,
                            self.max_source_bytes,
                            peak,
                            escape,
                        )?
                    }
                    SemanticInputConfig::FileLocalChunks { .. }
                    | SemanticInputConfig::CertifiedProjectionChunks { .. } => {
                        chunk_owner_memory_envelope(
                            self.input,
                            &envelope,
                            self.max_source_bytes,
                            *dimension,
                        )?
                    }
                };
                if reader_peak > read.need.memory_bytes
                    || owner_peak > resources.execution.enrichment.memory_bytes
                {
                    bail!(
                        "native OpenAI maximum input envelope exceeds reader or Enrichment memory reservation"
                    );
                }
                let parsed_url = reqwest::Url::parse(base_url)
                    .map_err(|_error| anyhow!("native OpenAI base URL is invalid"))?;
                let allowed_transport = parsed_url.scheme() == "https"
                    || (parsed_url.scheme() == "http"
                        && parsed_url.host_str() == Some("127.0.0.1")
                        && parsed_url.port().is_some());
                if !allowed_transport
                    || parsed_url.host_str().is_none()
                    || !parsed_url.username().is_empty()
                    || parsed_url.password().is_some()
                    || parsed_url.path() != "/"
                    || parsed_url.query().is_some()
                    || parsed_url.fragment().is_some()
                    || base_url.ends_with('/')
                {
                    bail!(
                        "native OpenAI endpoint must be an HTTPS origin or explicit loopback HTTP origin"
                    );
                }
                if egress.endpoint != format!("{base_url}/v1/embeddings") {
                    bail!("native OpenAI egress endpoint must match provider URL");
                }
                let grant = SemanticEgressGrantV1 {
                    tenant_id: tenant_id_hex(egress.tenant_id)?,
                    provider_id: "openai".into(),
                    endpoint: egress.endpoint.clone(),
                    region: egress.region.clone(),
                    retention: egress.retention.clone(),
                    model_id: format!("openai:{model}"),
                    model_revision: model_revision.clone(),
                    profile: egress.profile.clone(),
                    source_content_consent: egress.source_content_consent,
                };
                let policy = SemanticEgressPolicyV1::External(grant);
                let _query =
                    SemanticAdmissionEngine::admit(SemanticInputClass::QueryText, &policy)?;
                let _source =
                    SemanticAdmissionEngine::admit(SemanticInputClass::SourceContent, &policy)?;
                // NativeSemantic builds one owner vector per call and freezes
                // one provider unit as its declared adapter demand. A higher
                // per-call fanout cannot be funded by that execution grant.
                if *concurrency != 1
                    || resources.execution.interactive.provider_units == 0
                    || resources.execution.enrichment.provider_units == 0
                {
                    bail!("native OpenAI owner execution requires one funded attempt slot");
                }
                (policy, reader_escape.max(owner_escape))
            }
        };
        Ok(ValidatedSemanticConfig {
            scopes: Arc::new(scopes),
            descriptor: self.descriptor.descriptor(),
            provider: self.provider,
            egress_policy,
            max_source_bytes: self.max_source_bytes,
            input: self.input,
            read,
            provider_escaping_bytes,
        })
    }
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn tenant_id_hex(tenant: TenantId) -> Result<String> {
    let mut encoded = String::with_capacity(32);
    for byte in tenant.0 {
        std::fmt::Write::write_fmt(&mut encoded, format_args!("{byte:02x}"))
            .map_err(|_error| anyhow!("native tenant encoding failed"))?;
    }
    Ok(encoded)
}

/// Explicit operator policy; omission disables automatic queue draining.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceConfig {
    interval_ms: u64,
    max_steps_per_round: u32,
    child_limit: u32,
    work_limits: WorkLimitsConfig,
    round_timeout_ms: u64,
    head_probe_every: u32,
}

impl MaintenanceConfig {
    fn validate(self, plan: NativeResourcePlan, owned_pump: bool) -> Result<NativeReclaimConfig> {
        let config = NativeReclaimConfig {
            interval: Duration::from_millis(self.interval_ms),
            max_steps_per_round: self.max_steps_per_round,
            child_limit: self.child_limit,
            work_limits: self.work_limits.validate()?,
            round_timeout: Duration::from_millis(self.round_timeout_ms),
            head_probe_every: self.head_probe_every,
        };
        validate_automatic_reclaim(config, plan, owned_pump)?;
        Ok(config)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeConfig {
    workspace: WorkspaceKey,
    epoch: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedWorkConfig {
    scopes: Vec<ScopeConfig>,
    batch_limit: u32,
    max_in_flight: u32,
    poll_interval_ms: u64,
    dispatch_limits: WorkLimitsConfig,
    max_dispatches: u32,
    total_timeout_ms: u64,
    lease_ms: u64,
    retry_backoff_ms: u64,
}

impl OwnedWorkConfig {
    fn validate(self) -> Result<NativePumpConfig> {
        let config = NativePumpConfig {
            scopes: self
                .scopes
                .into_iter()
                .map(|scope| NativePumpScope {
                    workspace: scope.workspace,
                    epoch: scope.epoch,
                })
                .collect(),
            batch_limit: self.batch_limit,
            max_in_flight: self.max_in_flight,
            poll_interval: Duration::from_millis(self.poll_interval_ms),
            new_assignment: OwnedWorkPolicy {
                dispatch_limits: self.dispatch_limits.validate()?,
                max_dispatches: self.max_dispatches,
                total_timeout_ms: self.total_timeout_ms,
                lease_ms: self.lease_ms,
                retry_backoff_ms: self.retry_backoff_ms,
            },
            cadence: Arc::new(FixedPumpPolicy),
        };
        config.validate()?;
        Ok(config)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionConfig {
    scopes: Vec<ScopeConfig>,
    sequence_window: u64,
    page_limit: u32,
    interval_ms: u64,
    round_timeout_ms: u64,
    work_limits: WorkLimitsConfig,
    head_probe_every: u32,
}

impl RetentionConfig {
    fn validate(self) -> Result<NativeRetentionConfig> {
        let config = NativeRetentionConfig {
            scopes: self
                .scopes
                .into_iter()
                .map(|scope| NativeRetentionScope {
                    workspace: scope.workspace,
                    epoch: scope.epoch,
                })
                .collect(),
            sequence_window: self.sequence_window,
            page_limit: self.page_limit,
            interval: Duration::from_millis(self.interval_ms),
            round_timeout: Duration::from_millis(self.round_timeout_ms),
            work_limits: self.work_limits.validate()?,
            head_probe_every: self.head_probe_every,
        };
        config.validate()?;
        Ok(config)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadSessionsConfig {
    global_limit: usize,
    per_uid_limit: usize,
    terminal_ttl_millis: u64,
    sweep_limit: usize,
}
impl ReadSessionsConfig {
    fn validate(self) -> Result<crate::native::NativeReadSessionConfig> {
        Ok(crate::native::NativeReadSessionConfig {
            global_limit: self.global_limit,
            per_uid_limit: self.per_uid_limit,
            terminal_ttl: Duration::from_millis(self.terminal_ttl_millis),
            sweep_limit: self.sweep_limit,
        }
        .validate()?)
    }
}

struct ValidatedConfig {
    state_root: PathBuf,
    socket_path: PathBuf,
    limits: WorkLimits,
    resources: NativeServiceResources,
    literal: AdapterDescriptor,
    file_map: AdapterDescriptor,
    text: AdapterDescriptor,
    text_recipe: TextRecipe,
    bm25: Bm25Policy,
    text_need: ResourceNeed,
    semantic: Option<ValidatedSemanticConfig>,
    graph: Option<ValidatedGraphConfig>,
    maintenance: Option<NativeReclaimConfig>,
    retention: Option<NativeRetentionConfig>,
    read_sessions: Option<crate::native::NativeReadSessionConfig>,
    owned_work: Option<NativePumpConfig>,
    effort: Option<ValidatedEffortConfig>,
    overlay: Option<OverlayConfig>,
    stub_ast: Option<Vec<crate::native::StubGrammarPolicy>>,
}

impl NativeServeConfigV1 {
    fn validate(self) -> Result<ValidatedConfig> {
        if self.version != 1 {
            bail!("unsupported native config version {}", self.version);
        }
        if !self.state_root.is_absolute() || !self.socket_path.is_absolute() {
            bail!("native state root and socket path must be absolute");
        }
        if self.state_root == self.socket_path {
            bail!("native socket cannot be the state root");
        }
        let limits = self.work_limits.validate()?;
        let resources = self.resources.validate()?;
        let owned_work = self.owned_work.map(OwnedWorkConfig::validate).transpose()?;
        let effort = self.effort.map(EffortConfig::validate).transpose()?;
        let overlay = self.overlay.map(OverlayConfig::validate).transpose()?;
        let stub_ast = self.stub_ast.map(StubAstConfig::validate).transpose()?;
        if stub_ast.is_some() && overlay.is_none() {
            bail!("Native AST actions require a configured document overlay");
        }
        let semantic = self
            .semantic
            .map(|config| config.validate(resources))
            .transpose()?;
        let graph = self
            .graph
            .map(|config| config.validate(resources))
            .transpose()?;
        if owned_work.is_some() && semantic.is_none() {
            bail!("native owned work requires a configured Semantic owner adapter");
        }
        if let (Some(pump), Some(semantic)) = (&owned_work, &semantic)
            && pump
                .scopes
                .iter()
                .any(|scope| !semantic.scopes.contains(scope))
        {
            bail!("native owned work scope exceeds configured Semantic authorization");
        }
        let maintenance = self
            .maintenance
            .map(|config| config.validate(resources.execution, owned_work.is_some()))
            .transpose()?;
        let retention = self.retention.map(RetentionConfig::validate).transpose()?;
        let read_sessions = self
            .read_sessions
            .map(ReadSessionsConfig::validate)
            .transpose()?;
        if read_sessions.is_some() && maintenance.is_none() {
            bail!("native read sessions require automatic maintenance");
        }
        if retention.is_some() && maintenance.is_none() {
            bail!("native retention requires automatic reclaim configuration");
        }
        let read_session_bytes = read_sessions
            .as_ref()
            .map(crate::native_read_sessions::NativeReadSessions::required_memory)
            .transpose()?
            .unwrap_or(0);
        let escaping_bytes = semantic
            .as_ref()
            .map_or(0, |config| config.provider_escaping_bytes)
            .checked_add(read_session_bytes)
            .ok_or_else(|| anyhow!("native retained memory allowance overflow"))?;
        if escaping_bytes
            > resources
                .execution
                .retained_memory_headroom(owned_work.is_some(), maintenance.is_some())?
        {
            bail!(
                "native provider and read-session retained buffers require headroom beyond protected execution floors"
            );
        }
        let literal = self.literal.descriptor();
        let file_map = self.file_map.descriptor();
        let text = self.text.descriptor.descriptor();
        let unique = [literal.kind, file_map.kind, text.kind]
            .into_iter()
            .chain(semantic.as_ref().map(|config| config.descriptor.kind))
            .chain(graph.as_ref().map(|config| config.descriptor.kind))
            .collect::<BTreeSet<_>>();
        let expected_kinds = 3_usize
            .checked_add(usize::from(semantic.is_some()))
            .and_then(|count| count.checked_add(usize::from(graph.is_some())))
            .ok_or_else(|| anyhow!("native adapter kind count overflow"))?;
        if unique.len() != expected_kinds {
            bail!("native adapter kinds must be distinct");
        }
        let text_recipe = TextRecipe::new(Digest(self.text.admission_policy));
        let bm25 = Bm25Policy {
            k1: self.text.bm25.k1,
            b: self.text.bm25.b,
        };
        bm25.validate()?;
        let text_need: ResourceNeed = self.text.read_need.into();
        if text_need.cpu_units == 0
            || text_need.io_bytes == 0
            || text_need.memory_bytes == 0
            || text_need.provider_units != 0
            || !text_need.fits(resources.execution.interactive)
        {
            bail!("native text reader reservation exceeds Interactive envelope");
        }
        Ok(ValidatedConfig {
            state_root: self.state_root,
            socket_path: self.socket_path,
            limits,
            resources,
            literal,
            file_map,
            text,
            text_recipe,
            bm25,
            text_need,
            semantic,
            graph,
            maintenance,
            retention,
            read_sessions,
            owned_work,
            effort,
            overlay,
            stub_ast,
        })
    }
}

#[cfg(test)]
fn read_config(path: &Path) -> Result<ValidatedConfig> {
    read_config_checked(path, None)
}

fn read_config_checked(path: &Path, expected: Option<Digest>) -> Result<ValidatedConfig> {
    if !path.is_absolute() {
        bail!("native config path must be absolute");
    }
    let mut file = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .with_context(|| format!("open native config {}", path.display()))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        bail!("native config must be a regular file no larger than {MAX_CONFIG_BYTES} bytes");
    }
    let mut bytes = Vec::new();
    let _read_bytes = file
        .by_ref()
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).map_err(|_overflow| anyhow!("native config size overflow"))?
        > MAX_CONFIG_BYTES
    {
        bail!("native config exceeds {MAX_CONFIG_BYTES} bytes");
    }
    if let Some(expected) = expected
        && Digest(Sha256::digest(&bytes).into()) != expected
    {
        return Err(IndexError::DigestConflict.into());
    }
    let config: NativeServeConfigV1 = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode native config {}", path.display()))?;
    config.validate()
}

/// Admit native Semantic text at each provider call.
///
/// The same physical provider serves both paths, while each bridge admits the
/// concrete data class before outbound I/O. Startup admission alone cannot
/// reject tokenless text supplied later by a producer or query.
struct AdmittedTextProvider<P> {
    inner: Arc<P>,
    class: SemanticInputClass,
    egress_policy: SemanticEgressPolicyV1,
}

impl<P: TextEmbeddingProvider> AdmittedTextProvider<P> {
    fn admit(&self, texts: &[&str]) -> Result<(), CoreError> {
        for text in texts {
            let _admitted =
                SemanticAdmissionEngine::admit_text(self.class, text, &self.egress_policy)?;
        }
        Ok(())
    }
}

impl<P: TextEmbeddingProvider> TextEmbeddingProvider for AdmittedTextProvider<P> {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        self.admit(texts)?;
        self.inner.embed_batch(texts)
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        self.admit(texts)?;
        self.inner.embed_batch_within(texts, budget)
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn model_revision(&self) -> &str {
        self.inner.model_revision()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn normalization(&self) -> EmbeddingNormalization {
        self.inner.normalization()
    }

    fn supports_deterministic_payload_reuse(&self) -> bool {
        self.inner.supports_deterministic_payload_reuse()
    }
}

struct ScopedSemanticOwner<P> {
    inner: NativeSemantic<P>,
    scopes: Arc<BTreeSet<NativePumpScope>>,
}

impl<P: EmbeddingProvider> IndexOwnerBuildPort for ScopedSemanticOwner<P> {
    type Candidate = SemanticCandidate;

    fn validate_coverage(&self, coverage: &CoverageBinding) -> Result<(), IndexError> {
        self.inner.validate_coverage(coverage)
    }

    fn prepare_searchable<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<&'a CoverageBinding>,
        owners: MapRoot,
        materialized: Option<(OwnerKey, ObjectRef)>,
        object_maps: Option<&'a Arc<dyn quanta_index_core::index::ports::ObjectMapBatchPort>>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, quanta_index_core::index::SearchablePreparation> {
        let scope = NativePumpScope {
            workspace: context.attempt.operation.workspace,
            epoch: context.attempt.operation.epoch,
        };
        if !self.scopes.contains(&scope) {
            return Box::pin(async {
                Err(IndexError::InvalidInput(
                    "native Semantic source scope is not authorized".into(),
                ))
            });
        }
        self.inner
            .prepare_searchable(instance, prior, owners, materialized, object_maps, context)
    }

    fn resource_demand(&self) -> ResourceNeed {
        self.inner.resource_demand()
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.inner.validate_instance(instance)
    }

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        prior: Option<OwnerMemberBinding>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        let scope = NativePumpScope {
            workspace: context.attempt.operation.workspace,
            epoch: context.attempt.operation.epoch,
        };
        if !self.scopes.contains(&scope) {
            return Box::pin(async {
                Err(IndexError::InvalidInput(
                    "native Semantic source scope is not authorized".into(),
                ))
            });
        }
        self.inner
            .prepare_owner(instance, owner, input, prior, context)
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding> {
        let scope = NativePumpScope {
            workspace: context.attempt.operation.workspace,
            epoch: context.attempt.operation.epoch,
        };
        if !self.scopes.contains(&scope) {
            return Box::pin(async {
                Err(IndexError::InvalidInput(
                    "native Semantic source scope is not authorized".into(),
                ))
            });
        }
        self.inner.verify_owner(candidate, context)
    }
}

struct ScopedSemanticReader<P> {
    inner: NativeSemanticReader<P>,
    scopes: Arc<BTreeSet<NativePumpScope>>,
}

impl<P: TextEmbeddingProvider> IndexQueryPort<SemanticReadRequest, SearchResult>
    for ScopedSemanticReader<P>
{
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.inner.validate_instance(instance)
    }

    fn reservation(&self, request: &SemanticReadRequest) -> Result<ResourceNeed, IndexError> {
        self.inner.reservation(request)
    }

    fn query(
        &self,
        request: &SemanticReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<SearchResult>, IndexError> {
        let scope = NativePumpScope {
            workspace: context.ticket.view.identity.workspace,
            epoch: context.ticket.view.identity.epoch,
        };
        if !self.scopes.contains(&scope) {
            return Err(IndexError::InvalidInput(
                "native Semantic query scope is not authorized".into(),
            ));
        }
        self.inner.query(request, context)
    }
}

fn register_semantic_provider<P: TextEmbeddingProvider + 'static>(
    builders: &mut AdapterRegistry,
    readers: &mut QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    config: &ValidatedSemanticConfig,
    provider: P,
) -> Result<()> {
    let provider = Arc::new(provider);
    let owner_bridge = Arc::new(config.input.bridge(AdmittedTextProvider {
        inner: Arc::clone(&provider),
        class: SemanticInputClass::SourceContent,
        egress_policy: config.egress_policy.clone(),
    })?);
    let reader_bridge = Arc::new(config.input.bridge(AdmittedTextProvider {
        inner: provider,
        class: SemanticInputClass::QueryText,
        egress_policy: config.egress_policy.clone(),
    })?);
    let recipe = config
        .input
        .recipe(&owner_bridge, config.max_source_bytes)?;
    if config
        .input
        .recipe(&reader_bridge, config.max_source_bytes)?
        != recipe
    {
        bail!("native Semantic owner and reader recipes differ");
    }
    let ann_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .enable_all()
            .build()
            .context("native Semantic ANN runtime")?,
    );
    builders.register_owner(
        config.descriptor,
        ScopedSemanticOwner {
            inner: NativeSemantic::new(owner_bridge, recipe.clone())?.with_ann(),
            scopes: Arc::clone(&config.scopes),
        },
    )?;
    readers.register(
        config.descriptor,
        config.read.need,
        ScopedSemanticReader {
            inner: NativeSemanticReader::new(reader_bridge, recipe, config.read)?
                .with_ann(ann_runtime),
            scopes: Arc::clone(&config.scopes),
        },
    )?;
    Ok(())
}

fn register_semantic(
    builders: &mut AdapterRegistry,
    readers: &mut QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    config: &ValidatedSemanticConfig,
    service_provider_units: u64,
) -> Result<Arc<ProviderAttemptPool>> {
    let SemanticProviderConfig::OpenAi {
        model,
        model_revision,
        dimension,
        api_key_env,
        base_url,
        max_batch,
        max_estimated_tokens_per_request,
        max_retries,
        timeout_ms,
        max_response_bytes,
        concurrency,
        ..
    } = &config.provider;
    let api_key = std::env::var(api_key_env)
        .map_err(|_missing| anyhow!("native OpenAI API key environment variable is missing"))?;
    let provider_config =
        OpenAiProviderConfig::new(api_key, model.clone(), model_revision.clone(), *dimension)
            .with_base_url(base_url.clone())
            .with_max_batch(*max_batch)
            .with_max_estimated_tokens_per_request(*max_estimated_tokens_per_request)
            .with_max_retries(*max_retries)
            .with_timeout(Duration::from_millis(*timeout_ms))
            .with_max_response_bytes(*max_response_bytes)
            .with_concurrency(*concurrency);
    // Owner and query calls share this physical-attempt registry. Its bound
    // follows the service ledger, independently of each call's funded fanout.
    let attempt_capacity = usize::try_from(service_provider_units)
        .context("native provider capacity exceeds the platform size limit")?;
    let attempts = Arc::new(ProviderAttemptPool::new(attempt_capacity)?);
    let provider = OpenAiEmbeddingProvider::with_reqwest(provider_config)?
        .with_attempt_pool(Arc::clone(&attempts))
        .with_required_execution_custody();
    register_semantic_provider(
        builders,
        readers,
        config,
        L2UnitEmbeddingProvider::new(provider)?,
    )?;
    Ok(attempts)
}

struct NativeComposition {
    graph: Option<QueryAdapterRegistry<GraphQuery, GraphPage>>,
    repomap: Option<QueryAdapterRegistry<NativeRepoMapQuery, NativeRepoMapRankPage>>,
    capabilities: NativeServiceCapabilities,
    provider_signals: Option<Arc<crate::native_provider_signals::NativeProviderPoolSignals>>,
}

fn compose(config: &ValidatedConfig) -> Result<NativeComposition> {
    let mut builders = AdapterRegistry::new();
    let graph = config
        .graph
        .as_ref()
        .map(|graph| -> Result<_> {
            builders.register_projection(graph.descriptor, NativeGraph)?;
            let mut readers = QueryAdapterRegistry::new();
            readers.register(
                graph.descriptor,
                graph.read_need,
                NativeGraphReader::new(graph.read_need)?,
            )?;
            readers.validate_reservation(config.resources.execution.interactive)?;
            Ok(readers)
        })
        .transpose()?;
    let repomap = config
        .graph
        .as_ref()
        .map(|graph| -> Result<_> {
            let mut readers = QueryAdapterRegistry::new();
            readers.register(
                graph.descriptor,
                graph.read_need,
                NativeRepoMapRankReader::new(graph.read_need)?,
            )?;
            readers.validate_reservation(config.resources.execution.interactive)?;
            Ok(readers)
        })
        .transpose()?;
    builders.register(
        config.literal,
        NativeLiteralIndex,
        |delta| Ok(delta.clone()),
    )?;
    builders.register(config.file_map, NativeFileMap, |delta| Ok(delta.clone()))?;
    builders.register(
        config.text,
        NativeTextIndex::new(config.text_recipe),
        |delta| Ok(delta.clone()),
    )?;
    let mut text = QueryAdapterRegistry::<PlannedTextRead, TextReadResult>::new();
    text.register(
        config.text,
        config.text_need,
        NativeTextReader::new(config.text_recipe, config.bm25, config.text_need)?,
    )?;
    let reader_need = ResourceNeed {
        provider_units: 0,
        ..config.resources.execution.interactive
    };
    let mut literal = QueryAdapterRegistry::new();
    literal.register(
        config.literal,
        reader_need,
        NativeLiteralReader::new(reader_need)?,
    )?;
    let mut path = QueryAdapterRegistry::new();
    path.register(
        config.file_map,
        reader_need,
        NativePathReader::new(reader_need)?,
    )?;
    let mut semantic = QueryAdapterRegistry::<SemanticReadRequest, SearchResult>::new();
    let provider_attempt_pool = config
        .semantic
        .as_ref()
        .map(|semantic_config| {
            register_semantic(
                &mut builders,
                &mut semantic,
                semantic_config,
                config.resources.execution.capacity.provider_units,
            )
        })
        .transpose()?;
    let provider_signals = provider_attempt_pool.as_ref().map(|pool| {
        Arc::new(
            crate::native_provider_signals::NativeProviderPoolSignals::new(
                Arc::clone(pool),
                quanta_index_core::index::effort::ProviderMode::Remote,
            ),
        )
    });
    let capabilities = NativeServiceCapabilities {
        builders,
        literal,
        path,
        text,
        semantic,
        pump: config.owned_work.clone(),
        provider_attempt_pool,
    };
    let _needs = capabilities.validate_resources(
        config.resources,
        config.maintenance,
        config.retention.as_ref(),
    )?;
    Ok(NativeComposition {
        graph,
        repomap,
        capabilities,
        provider_signals,
    })
}

fn bind(
    config: &ValidatedConfig,
    composition: NativeComposition,
    authority: Option<crate::native::NativeBootAuthority>,
    request: &RequestBudgetV1,
) -> Result<NativeIndexService> {
    let NativeComposition {
        graph,
        repomap,
        capabilities,
        provider_signals,
    } = composition;
    if authority.is_none() && !config.state_root.exists() {
        let mut builder = fs::DirBuilder::new();
        let _private_mode = builder.mode(0o700);
        builder.create(&config.state_root)?;
    }
    let authority = match authority {
        Some(authority) => authority,
        None => crate::native::prepare_native_boot_authority(
            Arc::new(quanta_index_searchd::app::runtime::StateRootLease::acquire(
                &config.state_root,
            )?),
            &config.socket_path,
            config.resources,
            request,
        )?,
    };
    let service = NativeIndexService::bind_with_boot_authority(
        authority,
        &config.socket_path,
        config.limits,
        capabilities,
        config.resources,
        config.maintenance,
        config.retention.clone(),
    )?;
    if let Some(read_sessions) = config.read_sessions {
        service.configure_read_sessions(read_sessions)?;
    }
    if let Some(readers) = graph {
        service.configure_graph(readers)?;
    }
    if let Some(readers) = repomap {
        service.configure_repomap(readers)?;
    }
    let recorder = if config.overlay.is_some()
        || config
            .effort
            .is_some_and(|effort| effort.interaction_source.is_some())
    {
        let recorder = Arc::new(crate::native_interaction::NativeInteractionRecorder::new());
        service.configure_interaction_activity(Arc::clone(&recorder))?;
        Some(recorder)
    } else {
        None
    };
    if let Some(effort) = config.effort {
        let interaction: Option<
            Arc<dyn crate::native_auto_signals::NativeInteractionActivityPort>,
        > = if effort.interaction_source.is_some() {
            let recorder = recorder.as_ref().ok_or(IndexError::SnapshotUnavailable)?;
            Some(recorder.clone())
        } else {
            None
        };
        let mut signals =
            service.auto_signals(interaction, effort.worker_lag_max_age, effort.sample_ttl)?;
        if let Some(provider) = provider_signals {
            signals = signals.with_provider_signals(provider);
        }
        service.configure_effort(NativeEffortConfiguration {
            history: Arc::new(crate::native_effort_ports::NativeEffortObservationHistory::new()),
            policy: Arc::new(HysteresisPolicy::new(effort.policy)?),
            estimator: Arc::new(DeclaredCostEstimator),
            signals: Arc::new(signals),
        })?;
    }
    if let Some(overlay) = config.overlay {
        service.configure_overlay(
            overlay.sessions,
            overlay.documents,
            overlay.document_bytes,
            overlay.total_bytes,
            quanta_index_core::index::IndexInstanceId(overlay.text_instance),
        )?;
    }
    if let Some(policies) = &config.stub_ast {
        service.configure_published_stub(policies.clone())?;
    }
    Ok(service)
}

enum Event {
    Signal(i32),
    Done(Result<()>),
}

struct SignalWatch {
    handle: SignalHandle,
    join: Option<thread::JoinHandle<()>>,
}

impl Drop for SignalWatch {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(join) = self.join.take() {
            drop(join.join());
        }
    }
}

/// Run one explicit native product process. Return its exit status;
/// the caller owns the operator-facing stderr line for failed startup.
///
/// Process entry only: this sets the process umask and owns SIGINT/SIGTERM
/// handlers until exit. Signals cancel the original startup budget immediately;
/// the hard drain deadline starts once the service shutdown handle exists.
pub fn run(config_path: &Path) -> Result<u8> {
    run_command(&NativeStateCommandV1 {
        mode: NativeCommandModeV1::Serve,
        config_path: config_path.to_path_buf(),
        identity: None,
        config_digest: None,
        install_catalog_digest: None,
        select_operation: None,
        receipt_path: None,
    })
}

/// Execute one typed Native selection, offline verification or service command.
/// Selected service startup retains the verification lease into the listener.
pub fn run_command(command: &NativeStateCommandV1) -> Result<u8> {
    run_command_inner(command, None)
}

/// Run Native serve under one counted capture through boot and service drain.
/// Offline selection commands do not accept a service diagnostic capture.
pub fn run_command_with_capture(
    command: &NativeStateCommandV1,
    capture: quanta_index_searchd::DiagnosticCaptureLeaseV1,
) -> Result<u8> {
    anyhow::ensure!(
        command.mode == NativeCommandModeV1::Serve,
        "Native capture requires serve"
    );
    let capture = diagnostic::NativeCapture::new(capture);
    let result = run_command_inner(command, Some(&capture));
    capture.finish(&result);
    result
}

fn run_command_inner(
    command: &NativeStateCommandV1,
    capture: Option<&diagnostic::NativeCapture>,
) -> Result<u8> {
    let context = capture.and_then(diagnostic::NativeCapture::context);
    let boot = diagnostic::begin(context, quanta_index_core::DiagnosticStageV1::RuntimeBoot);
    command.validate()?;
    let config = read_config_checked(&command.config_path, command.config_digest)?;
    let _inherited_umask = quanta_index_searchd::app::harden_umask();
    let composition = compose(&config)?;
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    let signal_handle = signals.handle();
    let (tx, rx) = mpsc::channel();
    let signal_tx = tx.clone();
    let request = RequestBudgetV1::for_duration(quanta_index_searchd::HARD_DRAIN_DEADLINE);
    let request = match context {
        Some(context) => request.with_diagnostic_context(context.clone()),
        None => request,
    };
    let cancel = request.cancel_handle();
    let signal_thread = thread::Builder::new()
        .name("native-product-signals".into())
        .spawn(move || {
            for signal in signals.forever() {
                cancel.cancel();
                if signal_tx.send(Event::Signal(signal)).is_err() {
                    break;
                }
            }
        })?;
    let _signal_watch = SignalWatch {
        handle: signal_handle,
        join: Some(signal_thread),
    };
    if command.mode != NativeCommandModeV1::Serve {
        let authority = boot::execute(
            command,
            &config,
            &composition.capabilities.builders,
            request,
        )?;
        if authority.is_some() {
            bail!("offline Native command returned a service authority");
        }
        return Ok(0);
    }
    let authority = if command.identity.is_some() {
        Some(
            boot::execute(
                command,
                &config,
                &composition.capabilities.builders,
                request.clone(),
            )?
            .ok_or_else(|| anyhow!("selected Native serve has no verified authority"))?,
        )
    } else {
        None
    };
    let service = bind(&config, composition, authority, &request)?;
    let service = match context {
        Some(context) => service.with_diagnostic_context(context),
        None => service,
    };
    let object_io_observation = capture.and_then(|_| service.object_io_observation());
    diagnostic::finish(boot, &Ok::<(), anyhow::Error>(()));
    let shutdown = service.shutdown_handle();
    let service_thread = thread::Builder::new()
        .name("native-product-service".into())
        .spawn(move || {
            let result =
                match std::panic::catch_unwind(AssertUnwindSafe(|| service.run(ACCEPT_IDLE))) {
                    Ok(result) => result,
                    Err(_panic) => Err(anyhow!("native service thread panicked")),
                };
            drop(tx.send(Event::Done(result)));
        })?;
    let mut stopping = false;
    let mut hard_deadline: Option<Instant> = None;
    loop {
        let event = if let Some(deadline) = hard_deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("native service exceeded shutdown deadline");
            }
            rx.recv_timeout(remaining).map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    anyhow!("native service exceeded shutdown deadline")
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    anyhow!("native service exited without completion")
                }
            })?
        } else {
            rx.recv()
                .map_err(|_closed| anyhow!("native service exited without completion"))?
        };
        match event {
            Event::Signal(_signal) if !stopping => {
                stopping = true;
                shutdown.trigger();
                hard_deadline = Some(
                    Instant::now()
                        .checked_add(quanta_index_searchd::HARD_DRAIN_DEADLINE)
                        .ok_or_else(|| anyhow!("native shutdown deadline overflow"))?,
                );
            }
            Event::Signal(signal) => {
                let exit_code = 128_i32
                    .checked_add(signal)
                    .ok_or_else(|| anyhow!("native signal exit overflow"))?;
                return u8::try_from(exit_code)
                    .map_err(|_overflow| anyhow!("native signal exit overflow"));
            }
            Event::Done(result) => {
                service_thread
                    .join()
                    .map_err(|_panic| anyhow!("native service thread panicked after completion"))?;
                if let (Some(capture), Some(observer)) = (capture, &object_io_observation) {
                    capture.record_object_io(observer);
                }
                result?;
                if !stopping {
                    bail!("native listener stopped without operator shutdown");
                }
                return Ok(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, process::Command, sync::mpsc, thread, time::Duration};

    use super::{
        NativeCommandModeV1, NativeStateCommandV1, OverlayConfig, SemanticInputConfig,
        chunk_owner_memory_envelope, read_config, run_command,
    };
    use quanta_index_core::index::Digest;
    use quanta_index_embed::OpenAiProviderConfig;

    #[test]
    fn native_stub_ast_config_requires_explicit_unique_grammar_build_policy()
    -> Result<(), Box<dyn Error>> {
        let policy = serde_json::json!({
            "language": "java", "grammar_engine": "tree-sitter",
            "grammar_version": "0.23.5", "grammar_hash": null,
            "grammar_policy": vec![1_u8; 32]
        });
        let valid: super::StubAstConfig = serde_json::from_value(serde_json::json!({
            "policies": [&policy]
        }))?;
        if valid.validate()?.len() != 1 {
            return Err("valid AST policy count differs".into());
        }
        let mut unapproved = policy.clone();
        *unapproved
            .get_mut("grammar_policy")
            .ok_or("AST fixture grammar policy absent")? = serde_json::json!(vec![0_u8; 32]);
        for policies in [
            serde_json::json!([]),
            serde_json::json!([&policy, &policy]),
            serde_json::json!([unapproved]),
            serde_json::json!(vec![policy; 17]),
        ] {
            let invalid: super::StubAstConfig = serde_json::from_value(serde_json::json!({
                "policies": policies
            }))?;
            if invalid.validate().is_ok() {
                return Err("invalid AST policy was accepted".into());
            }
        }
        if serde_json::from_value::<super::StubAstConfig>(serde_json::json!({
            "policies": [], "implicit_trust": true
        }))
        .is_ok()
        {
            return Err("unknown AST trust field was accepted".into());
        }
        Ok(())
    }

    #[test]
    fn native_overlay_config_is_explicit_and_bounded() -> Result<(), Box<dyn Error>> {
        let valid = OverlayConfig {
            text_instance: [31; 16],
            sessions: 2,
            documents: 4,
            document_bytes: 1024,
            total_bytes: 4096,
        };
        let _validated = valid.validate()?;
        for invalid in [
            OverlayConfig {
                sessions: 0,
                ..valid
            },
            OverlayConfig {
                documents: 257,
                ..valid
            },
            OverlayConfig {
                document_bytes: 1025,
                total_bytes: 1024,
                ..valid
            },
            OverlayConfig {
                total_bytes: 17 << 20,
                ..valid
            },
        ] {
            if invalid.validate().is_ok() {
                return Err("Native overlay config accepted an invalid hard cap".into());
            }
        }
        Ok(())
    }

    #[test]
    fn native_semantic_input_modes_are_explicit_and_bounded() -> Result<(), Box<dyn Error>> {
        let source: SemanticInputConfig = serde_json::from_str(r#"{"kind":"source_utf8"}"#)?;
        source.validate(8)?;
        let local: SemanticInputConfig = serde_json::from_str(
            r#"{"kind":"file_local_chunks","max_utf8_bytes":128,"max_vectors":4}"#,
        )?;
        local.validate(8)?;
        let certified: SemanticInputConfig = serde_json::from_value(serde_json::json!({
            "kind": "certified_projection_chunks", "projection": vec![1_u8; 16],
            "producer": vec![2_u8; 16], "schema": vec![3_u8; 16], "max_vectors": 4
        }))?;
        certified.validate(8)?;
        for invalid in [
            r#"{"kind":"file_local_chunks","max_utf8_bytes":3,"max_vectors":4}"#,
            r#"{"kind":"file_local_chunks","max_utf8_bytes":128,"max_vectors":0}"#,
            r#"{"kind":"certified_projection_chunks","projection":[1,2],"producer":[2],"schema":[3],"max_vectors":4}"#,
        ] {
            let refused = serde_json::from_str::<SemanticInputConfig>(invalid)
                .map_or(true, |input| input.validate(8).is_err());
            if !refused {
                return Err(
                    format!("invalid Semantic input policy was accepted: {invalid}").into(),
                );
            }
        }
        if local.validate(100_000).is_ok() || certified.validate(100_000).is_ok() {
            return Err("Semantic vector coordinate cap was bypassed".into());
        }
        Ok(())
    }

    #[test]
    fn chunk_resource_admission_counts_members_and_prior_backing() -> Result<(), Box<dyn Error>> {
        let provider =
            OpenAiProviderConfig::new(String::new(), "model".into(), "revision".into(), 8)
                .with_max_response_bytes(65_536);
        let one = chunk_owner_memory_envelope(
            SemanticInputConfig::FileLocalChunks {
                max_utf8_bytes: 128,
                max_vectors: 1,
            },
            &provider,
            1024,
            8,
        )?;
        let four = chunk_owner_memory_envelope(
            SemanticInputConfig::FileLocalChunks {
                max_utf8_bytes: 128,
                max_vectors: 4,
            },
            &provider,
            1024,
            8,
        )?;
        if four.0 <= one.0 || four.1 <= one.1 {
            return Err("native Semantic chunk admission omitted aggregate member backing".into());
        }
        Ok(())
    }

    #[test]
    fn public_native_command_refuses_invalid_mode_before_config_open() -> Result<(), Box<dyn Error>>
    {
        let temp = tempfile::tempdir()?;
        let absent = temp.path().join("absent-native-config.json");
        let invalid = NativeStateCommandV1 {
            mode: NativeCommandModeV1::Serve,
            config_path: absent.clone(),
            identity: None,
            config_digest: Some(Digest([1; 32])),
            install_catalog_digest: None,
            select_operation: None,
            receipt_path: None,
        };
        let error = match run_command(&invalid) {
            Ok(_status) => return Err("plain Serve with digest was accepted".into()),
            Err(error) => error,
        };
        if !error.to_string().contains("serve-native") {
            return Err(format!("expected serve-native refusal: {error}").into());
        }
        if absent.exists() {
            return Err("invalid command opened or created config".into());
        }
        Ok(())
    }

    #[test]
    fn fifo_config_is_refused_by_the_product_reader_without_blocking() -> Result<(), Box<dyn Error>>
    {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("native-config.fifo");
        if !Command::new("mkfifo").arg(&path).status()?.success() {
            return Err("mkfifo fixture setup failed".into());
        }
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _sent = tx.send(
                read_config(&path)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            );
        });
        let result = rx.recv_timeout(Duration::from_secs(3))?;
        worker
            .join()
            .map_err(|_panic| std::io::Error::other("native config reader panicked"))?;
        match result {
            Ok(()) => Err("FIFO config unexpectedly accepted".into()),
            Err(error) if error.contains("regular file") => Ok(()),
            Err(error) => {
                Err(format!("FIFO refusal lacked regular-file diagnosis: {error}").into())
            }
        }
    }
}
