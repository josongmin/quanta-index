//! Native incremental index identities. Source, view, operation and attempt
//! namespaces are deliberately distinct; sequence labels never prove freshness.

macro_rules! id {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub [u8; 16]);
        crate::native_serde_newtype!($name, [u8; 16]);
    )+};
}
id!(
    TenantId,
    CollectionId,
    WorktreeId,
    SlotId,
    ProducerId,
    StreamId,
    StableOwnerId,
    IndexInstanceId,
    IndexKind,
    ProjectionId,
    SchemaId,
    StorageFormatId,
    PublicationGroupId,
    AttemptId,
    TicketId
);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Digest(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectRef(pub Digest);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceKey {
    pub tenant: TenantId,
    pub collection: CollectionId,
    pub worktree: WorktreeId,
    pub slot: SlotId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewRef {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
    pub sequence: u64,
    pub digest: Digest,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OwnerKey {
    pub producer: ProducerId,
    pub stable_id: StableOwnerId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperationKey {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
    pub producer: ProducerId,
    pub stream: StreamId,
    pub stream_epoch: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttemptRef {
    pub operation: OperationKey,
    pub attempt: AttemptId,
    pub fence: u64,
}

crate::native_serde_newtype!(Digest, [u8; 32]);
crate::native_serde_newtype!(ObjectRef, Digest);
crate::native_serde_struct!(WorkspaceKey {
    tenant: TenantId,
    collection: CollectionId,
    worktree: WorktreeId,
    slot: SlotId
});
crate::native_serde_struct!(ViewRef {
    workspace: WorkspaceKey,
    epoch: u64,
    sequence: u64,
    digest: Digest
});
crate::native_serde_struct!(OwnerKey {
    producer: ProducerId,
    stable_id: StableOwnerId
});
crate::native_serde_struct!(OperationKey {
    workspace: WorkspaceKey,
    epoch: u64,
    producer: ProducerId,
    stream: StreamId,
    stream_epoch: u64,
    sequence: u64
});
crate::native_serde_struct!(AttemptRef {
    operation: OperationKey,
    attempt: AttemptId,
    fence: u64
});
