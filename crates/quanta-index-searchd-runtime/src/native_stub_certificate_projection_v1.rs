//! Selected-view AST certificate reader for Native Stub requests.
//!
//! The projection is the authority; a request manifest is only a claim to be
//! checked against its immutable per-source output. No parser runs here.

use std::sync::Arc;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    index::{Digest, OwnerKey, ProjectionBinding, ViewSelection},
    native_protocol::{
        MAX_NATIVE_STUB_CERTIFICATE_BYTES, NATIVE_STUB_AST_PRODUCER, NATIVE_STUB_AST_PROJECTION,
        NATIVE_STUB_AST_SCHEMA, NativeStubCertificate, NativeStubCertificateHeader,
        native_stub_certificate_policy,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLifetime, StoredObject, WorkBudget, WorkUsage, decode_projection_owner,
    owner_map_key,
    ports::{ImmutableObjectPort, PersistentMapPort},
};
use quanta_index_kernel::{Kernel, ReadTicket};
use quanta_index_lexical::native::stub::{
    StubAstAttestation, StubAstAttestationSession, StubAstCertificatePort, StubCapability,
    StubContext, StubItem, StubRefusal,
};
use quanta_index_storage::FileObjectStore;

const MAX_POLICIES: usize = 16;

/// A deployment-approved parser/grammar identity, not a value inferred from
/// an untrusted projection output. The build provenance is bound by the
/// explicit nonzero policy digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StubGrammarPolicy {
    pub language: String,
    pub grammar_engine: String,
    pub grammar_version: String,
    pub grammar_hash: Option<[u8; 8]>,
    pub grammar_policy: Digest,
}

pub(crate) struct PublishedStubAstCertificatePort {
    kernel: Arc<Kernel<SqliteCatalog>>,
    objects: Arc<FileObjectStore>,
    policies: Vec<StubGrammarPolicy>,
}

pub(crate) fn validate_stub_grammar_policies(
    policies: &[StubGrammarPolicy],
) -> Result<(), IndexError> {
    if policies.is_empty() || policies.len() > MAX_POLICIES {
        return Err(IndexError::InvalidInput(
            "Native Stub requires bounded grammar policies".into(),
        ));
    }
    for (at, policy) in policies.iter().enumerate() {
        if policy.language.is_empty()
            || policy.language.len() > 64
            || policy.language.contains('\0')
            || policy.grammar_engine.is_empty()
            || policy.grammar_engine.len() > 32
            || policy.grammar_engine.contains('\0')
            || policy.grammar_version.is_empty()
            || policy.grammar_version.len() > 128
            || policy.grammar_version.contains('\0')
            || policy.grammar_policy == Digest([0; 32])
            || policies
                .iter()
                .take(at)
                .any(|prior| prior.language == policy.language)
        {
            return Err(IndexError::InvalidInput(
                "Native Stub grammar policy is invalid or duplicate".into(),
            ));
        }
    }
    Ok(())
}

impl PublishedStubAstCertificatePort {
    pub(crate) fn new(
        kernel: Arc<Kernel<SqliteCatalog>>,
        objects: Arc<FileObjectStore>,
        mut policies: Vec<StubGrammarPolicy>,
    ) -> Result<Self, IndexError> {
        validate_stub_grammar_policies(&policies)?;
        policies.sort_unstable_by(|left, right| left.language.cmp(&right.language));
        Ok(Self {
            kernel,
            objects,
            policies,
        })
    }
}

struct CertifiedItem {
    stable_id: Digest,
    ast_digest: Digest,
    start: u32,
    end: u32,
    label: String,
}

struct PublishedSession {
    items: Vec<CertifiedItem>,
    // The physical object and all copied fields stay live through preflight.
    _backing: StoredObject,
    ticket: ReadTicket,
}

impl StubAstAttestationSession for PublishedSession {
    fn capability(&self) -> StubCapability {
        StubCapability {
            stub: true,
            action: true,
        }
    }

    fn attest_item(&self, item: &StubItem, budget: &mut WorkBudget) -> Result<(), StubRefusal> {
        budget.checkpoint()?;
        self.ticket
            .validate()
            .map_err(StubRefusal::from_selected_view_error)?;
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let at = self
            .items
            .binary_search_by_key(&item.stable_id, |value| value.stable_id)
            .map_err(|_insertion_point| StubRefusal::StaleBinding)?;
        let certified = self.items.get(at).ok_or(StubRefusal::StaleBinding)?;
        if certified.ast_digest != item.ast_digest
            || certified.start != item.span.start
            || certified.end != item.span.end
            || certified.label != item.label
        {
            return Err(StubRefusal::StaleBinding);
        }
        Ok(())
    }
}

fn verify_certificate_header(
    header: &NativeStubCertificateHeader<'_>,
    binding: &ProjectionBinding,
    owner: OwnerKey,
    context: &StubContext,
    exact_source: &str,
    policy: &StubGrammarPolicy,
) -> Result<(), StubRefusal> {
    if header.projection != binding.projection
        || header.producer != binding.producer
        || header.schema != binding.schema
        || header.producer_policy != binding.producer_policy
        || header.owner != owner
        || header.source_owner != context.owner
        || header.source != context.source
        || usize::try_from(header.source_bytes).map_err(|_error| StubRefusal::InvalidManifest)?
            != exact_source.len()
        || header.language != policy.language
        || header.grammar_engine != policy.grammar_engine
        || header.grammar_version != policy.grammar_version
        || header.grammar_hash != policy.grammar_hash
        || header.grammar_policy != policy.grammar_policy
    {
        return Err(StubRefusal::StaleBinding);
    }
    Ok(())
}

impl StubAstCertificatePort for PublishedStubAstCertificatePort {
    fn begin(
        &self,
        context: &StubContext,
        exact_source: &str,
        budget: &mut WorkBudget,
    ) -> Result<StubAstAttestation, StubRefusal> {
        budget.checkpoint()?;
        let at = self
            .policies
            .binary_search_by(|item| item.language.as_str().cmp(&context.language))
            .map_err(|_insertion_point| {
                StubRefusal::UnsupportedLanguage(context.language.clone())
            })?;
        let policy = self
            .policies
            .get(at)
            .ok_or_else(|| StubRefusal::UnsupportedLanguage(context.language.clone()))?;
        let ticket = self
            .kernel
            .pin_view(ViewSelection::Exact(context.view), budget)
            .map_err(StubRefusal::from_selected_view_error)?;
        let result = (|| {
            ticket
                .validate()
                .map_err(StubRefusal::from_selected_view_error)?;
            if ticket.view().identity != context.view
                || ticket.view().identity.workspace != context.workspace
            {
                return Err(StubRefusal::StaleBinding);
            }
            // The Kernel reader validates the binding object's canonical edges;
            // Exact(view) remains pinned by this outer ticket throughout.
            let (read_view, binding) = self
                .kernel
                .read_projection_binding(
                    ViewSelection::Exact(context.view),
                    NATIVE_STUB_AST_PROJECTION,
                    budget,
                )
                .map_err(StubRefusal::from_selected_view_error)?;
            let binding = binding.ok_or(StubRefusal::StaleBinding)?;
            if read_view != context.view
                || binding.projection != NATIVE_STUB_AST_PROJECTION
                || binding.producer != NATIVE_STUB_AST_PRODUCER
                || binding.schema != NATIVE_STUB_AST_SCHEMA
                || binding.producer_policy != native_stub_certificate_policy()
                || binding.source != ticket.view().source
            {
                return Err(StubRefusal::StaleBinding);
            }
            let selected_source = self
                .objects
                .lookup_object(
                    ticket.view().source.owners,
                    &owner_map_key(context.owner),
                    budget,
                )
                .map_err(StubRefusal::from_selected_view_error)?;
            if selected_source != Some(context.source) {
                return Err(StubRefusal::StaleBinding);
            }
            let owner = OwnerKey {
                producer: NATIVE_STUB_AST_PRODUCER,
                stable_id: context.owner.stable_id,
            };
            let reference = self
                .objects
                .lookup_object(binding.owners, &owner_map_key(owner), budget)
                .map_err(StubRefusal::from_selected_view_error)?
                .ok_or(StubRefusal::StaleBinding)?;
            // FileObjectStore authenticates the full canonical wire, whose
            // registered byte size also includes framing and child references.
            let envelope = self
                .objects
                .get(reference, budget)
                .map_err(StubRefusal::from_selected_view_error)?;
            let contribution = decode_projection_owner(&envelope.payload, budget)
                .map_err(StubRefusal::from_selected_view_error)?;
            let output_ref = contribution.output.ok_or(StubRefusal::StaleBinding)?;
            let mut expected_edges = [context.source, output_ref];
            expected_edges.sort_unstable();
            if envelope.registration.reference != reference
                || envelope.registration.bytes
                    < u64::try_from(envelope.payload.len())
                        .map_err(|_error| StubRefusal::Resource(IndexError::ResourceLimited))?
                || context.source == output_ref
                || envelope.registration.children.as_slice() != expected_edges.as_slice()
                || contribution.projection != binding.projection
                || contribution.producer != binding.producer
                || contribution.owner != owner
                || contribution.source_owner != Some(context.owner)
                || contribution.source != Some(context.source)
            {
                return Err(StubRefusal::StaleBinding);
            }
            let output = self
                .objects
                .get(output_ref, budget)
                .map_err(StubRefusal::from_selected_view_error)?;
            if output.registration.reference != output_ref
                || output.registration.bytes
                    < u64::try_from(output.payload.len())
                        .map_err(|_error| StubRefusal::Resource(IndexError::ResourceLimited))?
                || output.registration.children.as_slice() != [context.source]
                || output.payload.len() > MAX_NATIVE_STUB_CERTIFICATE_BYTES
            {
                return Err(StubRefusal::InvalidManifest);
            }
            budget.checkpoint()?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: u64::try_from(output.payload.len())
                    .map_err(|_error| StubRefusal::Resource(IndexError::ResourceLimited))?,
                ..WorkUsage::default()
            })?;
            let certificate = NativeStubCertificate::parse(&output.payload)
                .map_err(|_error| StubRefusal::InvalidManifest)?;
            verify_certificate_header(
                &certificate.header(),
                &binding,
                owner,
                context,
                exact_source,
                policy,
            )?;
            // Parse has checked every row, including order and trailing bytes.
            // Payload length conservatively bounds all copied label bytes, so
            // allocation is admitted before the one item decode/materialization.
            let bytes = certificate
                .len()
                .checked_mul(std::mem::size_of::<CertifiedItem>())
                .and_then(|n| n.checked_add(output.payload.len()))
                .and_then(|n| n.checked_add(std::mem::size_of::<PublishedSession>()))
                .ok_or(StubRefusal::Resource(IndexError::ResourceLimited))?;
            let bytes = u64::try_from(bytes)
                .map_err(|_error| StubRefusal::Resource(IndexError::ResourceLimited))?;
            budget.charge(WorkUsage {
                bytes,
                reads: 1,
                ..WorkUsage::default()
            })?;
            let memory = budget
                .reserve_memory(bytes, MemoryLifetime::Retained)?
                .ok_or_else(|| {
                    StubRefusal::Resource(IndexError::BackendViolation(
                        "Native Stub certificate needs retained memory scope".into(),
                    ))
                })?;
            let mut items = Vec::new();
            items
                .try_reserve_exact(certificate.len())
                .map_err(|_error| StubRefusal::Resource(IndexError::ResourceLimited))?;
            for row in certificate.iter() {
                budget.checkpoint()?;
                let row = row.map_err(|_error| StubRefusal::InvalidManifest)?;
                let mut label = String::new();
                label
                    .try_reserve_exact(row.label.len())
                    .map_err(|_error| StubRefusal::Resource(IndexError::ResourceLimited))?;
                label.push_str(row.label);
                items.push(CertifiedItem {
                    stable_id: row.stable_id,
                    ast_digest: row.ast_digest,
                    start: row.start,
                    end: row.end,
                    label,
                });
            }
            ticket
                .validate()
                .map_err(StubRefusal::from_selected_view_error)?;
            Ok((items, output, memory))
        })();
        match result {
            Err(error) => {
                drop(ticket.release());
                Err(error)
            }
            Ok((items, output, memory)) => Ok(StubAstAttestation::new(
                Box::new(PublishedSession {
                    items,
                    _backing: output,
                    ticket,
                }),
                memory,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        index::{
            CollectionId, MapRoot, ObjectRef, ProducerId, ProjectionId, SlotId, SourceCut,
            StableOwnerId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
        },
        native_protocol::{
            NativeStubCertificateItem, encode_native_stub_certificate_into,
            native_stub_certificate_encoded_len,
        },
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(format!("condition failed: {}", stringify!($condition)).into());
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let (left, right) = (&$left, &$right);
            if left != right {
                return Err(format!("values differ: {left:?} != {right:?}").into());
            }
        }};
    }

    type Fixture = (
        StubContext,
        ProjectionBinding,
        OwnerKey,
        StubGrammarPolicy,
        Vec<u8>,
    );

    fn fixture() -> TestResult<Fixture> {
        let source_owner = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        };
        let owner = OwnerKey {
            producer: NATIVE_STUB_AST_PRODUCER,
            stable_id: source_owner.stable_id,
        };
        let workspace = WorkspaceKey {
            tenant: TenantId([3; 16]),
            collection: CollectionId([4; 16]),
            worktree: WorktreeId([5; 16]),
            slot: SlotId([6; 16]),
        };
        let context = StubContext {
            workspace,
            publisher: quanta_index_contract::native_protocol::NativeInteractionPublisherId(
                [7; 32],
            ),
            session_epoch: 1,
            owner: source_owner,
            view: ViewRef {
                workspace,
                epoch: 1,
                sequence: 2,
                digest: Digest([8; 32]),
            },
            source: ObjectRef(Digest([9; 32])),
            document_revision: 1,
            language: "java".into(),
        };
        let binding = ProjectionBinding {
            projection: NATIVE_STUB_AST_PROJECTION,
            producer: NATIVE_STUB_AST_PRODUCER,
            owners: MapRoot::default(),
            source: SourceCut {
                owners: MapRoot::default(),
                paths: MapRoot::default(),
                frontiers: MapRoot::default(),
            },
            schema: NATIVE_STUB_AST_SCHEMA,
            producer_policy: native_stub_certificate_policy(),
            dependency_context: Digest([10; 32]),
            certificate: ObjectRef(Digest([11; 32])),
        };
        let policy = StubGrammarPolicy {
            language: "java".into(),
            grammar_engine: "tree-sitter".into(),
            grammar_version: "0.23.4".into(),
            grammar_hash: Some([12; 8]),
            grammar_policy: Digest([13; 32]),
        };
        let header = NativeStubCertificateHeader {
            projection: binding.projection,
            producer: binding.producer,
            schema: binding.schema,
            producer_policy: binding.producer_policy,
            owner,
            source_owner,
            source: context.source,
            source_bytes: 8,
            language: &policy.language,
            grammar_engine: &policy.grammar_engine,
            grammar_version: &policy.grammar_version,
            grammar_hash: policy.grammar_hash,
            grammar_policy: policy.grammar_policy,
        };
        let item = NativeStubCertificateItem {
            stable_id: Digest([14; 32]),
            ast_digest: Digest([15; 32]),
            start: 0,
            end: 8,
            label: "x",
        };
        let mut wire = vec![0; native_stub_certificate_encoded_len(&header, &[item])?];
        encode_native_stub_certificate_into(&header, &[item], &mut wire)?;
        Ok((context, binding, owner, policy, wire))
    }

    #[test]
    fn real_wire_header_requires_exact_source_projection_and_grammar() -> TestResult {
        let (context, binding, owner, policy, wire) = fixture()?;
        let certificate = NativeStubCertificate::parse(&wire)?;
        check_eq!(certificate.len(), 1);
        let header = certificate.header();
        check!(
            verify_certificate_header(&header, &binding, owner, &context, "let x=1;", &policy)
                .is_ok()
        );
        check!(matches!(
            verify_certificate_header(&header, &binding, owner, &context, "short", &policy),
            Err(StubRefusal::StaleBinding)
        ));
        let mut foreign_context = context.clone();
        foreign_context.source = ObjectRef(Digest([99; 32]));
        check!(matches!(
            verify_certificate_header(
                &header,
                &binding,
                owner,
                &foreign_context,
                "let x=1;",
                &policy
            ),
            Err(StubRefusal::StaleBinding)
        ));
        let mut foreign_binding = binding;
        foreign_binding.projection = ProjectionId([16; 16]);
        check!(matches!(
            verify_certificate_header(
                &header,
                &foreign_binding,
                owner,
                &context,
                "let x=1;",
                &policy
            ),
            Err(StubRefusal::StaleBinding)
        ));
        let mut foreign_policy = policy;
        foreign_policy.grammar_policy = Digest([17; 32]);
        check!(matches!(
            verify_certificate_header(
                &header,
                &binding,
                owner,
                &context,
                "let x=1;",
                &foreign_policy
            ),
            Err(StubRefusal::StaleBinding)
        ));
        let (_, truncated) = wire.split_last().ok_or("missing fixture bytes")?;
        check!(NativeStubCertificate::parse(truncated).is_err());
        Ok(())
    }

    #[test]
    fn grammar_registration_requires_bounded_unique_explicit_policy() -> TestResult {
        let (_, _, _, policy, _) = fixture()?;
        check!(validate_stub_grammar_policies(std::slice::from_ref(&policy)).is_ok());
        check!(validate_stub_grammar_policies(&[policy.clone(), policy.clone()]).is_err());
        let mut unknown = policy;
        unknown.grammar_policy = Digest([0; 32]);
        check!(validate_stub_grammar_policies(&[unknown]).is_err());
        Ok(())
    }
}
