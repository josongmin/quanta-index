//! Independent ordinal/visibility oracles for the union caller. Physical wire
//! and grant behavior is tested through the real Storage port separately.

use super::*;
use quanta_index_core::index::object_install::PreparedObjectInstallRun;
use quanta_index_core::index::{MemoryLease, MemoryLifetime, StagedObjectReferences};

#[path = "prepared_union_custody.rs"]
mod custody_tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mode {
    Complete,
    Decline,
    WrongLast,
    Cancel,
    Fail,
}

struct Prepared<'a> {
    owner: &'a TestObjects,
    attempt: quanta_index_core::index::AttemptRef,
    mode: Mode,
    references: Vec<ObjectRef>,
    maximum_payload: usize,
    packet_bytes: usize,
    used: usize,
    second: bool,
    memory: Option<MemoryLease>,
    _box_memory: Option<MemoryLease>,
}

pub(super) fn prepare<'a>(
    owner: &'a TestObjects,
    attempt: quanta_index_core::index::AttemptRef,
    maximum_payload: usize,
    packet_bytes: usize,
    budget: &WorkBudget,
) -> Result<Option<Box<dyn PreparedObjectInstallRun + 'a>>, IndexError> {
    let Some(mode) = owner.union_install else {
        return Ok(None);
    };
    if mode == Mode::Decline {
        return Ok(None);
    }
    let Some(memory) =
        budget.try_reserve_memory_for_optimization(8192, MemoryLifetime::Retained)?
    else {
        return Ok(None);
    };
    let Some(box_memory) = budget.try_reserve_memory_for_optimization(
        u64::try_from(std::mem::size_of::<Prepared<'_>>())
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?
    else {
        return Ok(None);
    };
    let mut references = Vec::new();
    references
        .try_reserve_exact(256)
        .map_err(|_error| IndexError::ResourceLimited)?;
    Ok(Some(Box::new(Prepared {
        owner,
        attempt,
        mode,
        references,
        maximum_payload,
        packet_bytes,
        used: 0,
        second: false,
        memory: Some(memory),
        _box_memory: Some(box_memory),
    })))
}

impl PreparedObjectInstallRun for Prepared<'_> {
    fn accepts(&self, bytes: usize) -> bool {
        self.references.len() < 256
            && bytes <= self.maximum_payload
            && self
                .used
                .checked_add(bytes)
                .is_some_and(|next| next <= self.packet_bytes || !self.second)
    }

    fn push(
        &mut self,
        payload: &[u8],
        expected: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        check!(self.accepts(payload.len()));
        check_eq!(ObjectRef(Digest(Sha256::digest(payload).into())), expected);
        let next = self
            .used
            .checked_add(payload.len())
            .ok_or(IndexError::ResourceLimited)?;
        if next > self.packet_bytes {
            self.second = true;
            self.used = payload.len();
        } else {
            self.used = next;
        }
        self.references.push(expected);
        Ok(())
    }

    fn check_payload(&self, slot: usize, payload: &[u8]) -> Result<(), IndexError> {
        check_eq!(
            self.references.get(slot),
            Some(&ObjectRef(Digest(Sha256::digest(payload).into())))
        );
        Ok(())
    }

    fn references(&self) -> &[ObjectRef] {
        &self.references
    }

    fn install(
        mut self: Box<Self>,
        attempt: quanta_index_core::index::AttemptRef,
        _sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedObjectReferences, IndexError> {
        check_eq!(attempt, self.attempt);
        let _prior = self.owner.install_batches.fetch_add(1, Ordering::Relaxed);
        let _prior = self
            .owner
            .install_batch_inputs
            .fetch_add(self.references.len(), Ordering::Relaxed);
        match self.mode {
            Mode::WrongLast => {
                *self
                    .references
                    .last_mut()
                    .ok_or(IndexError::ResourceLimited)? = ObjectRef(Digest([0xff; 32]))
            }
            Mode::Cancel => budget.request().cancel_handle().cancel(),
            Mode::Fail => {
                return Err(IndexError::Storage(
                    "injected union installation failure".into(),
                ));
            }
            Mode::Complete | Mode::Decline => {}
        }
        let Prepared {
            references, memory, ..
        } = *self;
        StagedObjectReferences::from_parts(references, memory)
    }
}

struct UnionMetadataRefusal {
    memory: Arc<TokenMemory>,
    bytes: u64,
    denied: Arc<AtomicUsize>,
}

impl quanta_index_core::index::MemoryScopePort for UnionMetadataRefusal {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if lifetime == MemoryLifetime::Temporary && bytes == self.bytes {
            let _previous = self.denied.fetch_add(1, Ordering::Relaxed);
            return Err(IndexError::ResourceLimited);
        }
        token_memory_grant(&self.memory, bytes)
    }
}

#[test]
fn union_keeps_every_ordinal_planned_until_all_outputs_match_or_original_fallback_finishes()
-> Result<(), IndexError> {
    for (mode, limited) in [
        (Mode::Complete, false),
        (Mode::Decline, false),
        (Mode::Complete, true),
        (Mode::WrongLast, false),
        (Mode::Cancel, false),
        (Mode::Fail, false),
    ] {
        let mut fixture = visitor_fixture()?;
        fixture.objects.union_install = Some(mode);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let memory = Arc::new(TokenMemory {
            ceiling: 16 * 1024 * 1024,
            ..TokenMemory::default()
        });
        // Match the existing 256-input install-envelope fixture; this is
        // independent of product request caps and the 190-row packet helper.
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 4096,
                writes: 4096,
                bytes: 16_000_000,
                mutations: 4096,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let denied = Arc::new(AtomicUsize::new(0));
        let scope: Arc<dyn quanta_index_core::index::MemoryScopePort> = if limited {
            Arc::new(UnionMetadataRefusal {
                memory: Arc::clone(&memory),
                bytes: OccurrenceWrites::metadata_bytes(
                    quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH,
                )?,
                denied: Arc::clone(&denied),
            })
        } else {
            Arc::new(BatchOutputMemoryScope(Arc::clone(&memory)))
        };
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(scope)));
        let spool = TextSpool::new(&scratch, &mut work)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut writes = OccurrenceWrites::new();
        writes.pack_owners = true;
        writes.install_objects = Some(&fixture.objects);
        let mut edits = InventoryEdits::new(0)?;
        let mut confirmed = ConfirmedOccurrences::new(64)?;
        let mut expected = Vec::new();
        let mut expected_rows = BTreeMap::new();
        for ordinal in 0..256u16 {
            let mut bytes = vec![0x5a; 8192];
            bytes
                .get_mut(..2)
                .ok_or(IndexError::ResourceLimited)?
                .copy_from_slice(&ordinal.to_be_bytes());
            let reference = ObjectRef(Digest(Sha256::digest(&bytes).into()));
            expected.push(reference);
            let key = format!("\0term-{ordinal:03}").into_bytes();
            let _previous = expected_rows.insert((u32::from(ordinal), key.clone()), reference);
            let payload = super::super::super::text_memory::PreparedPayload::from_staged(
                quanta_index_core::index::StagedPayload::copy(&bytes, &work)?,
            )?;
            let mut context = AttemptContext {
                attempt,
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            writes.flush_before_input(
                reference,
                payload.as_slice().len(),
                &mut context,
                &spool,
                &mut confirmed,
                &mut edits,
            )?;
            writes.push(
                OccurrenceEdit {
                    ordinal: u32::from(ordinal),
                    key: PreparedKey::copy(&key, context.budget)?,
                    prior: None,
                    identified: reference,
                },
                payload,
                &spool,
                &mut edits,
                context.budget,
            )?;
        }
        if writes.prepared_run.is_some() {
            // A selector in the second physical packet aliases the first
            // packet's wire without another object or a partial confirmation.
            let mut bytes = vec![0x5a; 8192];
            bytes
                .get_mut(..2)
                .ok_or(IndexError::ResourceLimited)?
                .copy_from_slice(&0u16.to_be_bytes());
            let reference = *expected.first().ok_or(IndexError::ResourceLimited)?;
            let payload = super::super::super::text_memory::PreparedPayload::from_staged(
                quanta_index_core::index::StagedPayload::copy(&bytes, &work)?,
            )?;
            writes.push(
                OccurrenceEdit {
                    ordinal: 200,
                    key: PreparedKey::copy(b"\0alias-first", &work)?,
                    prior: None,
                    identified: reference,
                },
                payload,
                &spool,
                &mut edits,
                &mut work,
            )?;
            let _previous = expected_rows.insert((200, b"\0alias-first".to_vec()), reference);
        }
        check!(spool.inventory_cursor(256)?.next_edit(&mut work).is_err());
        let result = writes.flush(
            &mut AttemptContext {
                attempt,
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            },
            &spool,
            &mut confirmed,
            &mut edits,
        );
        match mode {
            Mode::Complete | Mode::Decline => {
                result?;
                check_eq!(
                    fixture.objects.install_batches.load(Ordering::Relaxed),
                    if mode == Mode::Decline || limited {
                        2
                    } else {
                        1
                    }
                );
                check_eq!(
                    fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
                    256
                );
                let mut cursor = spool.inventory_cursor(256)?;
                for ((ordinal, key), reference) in &expected_rows {
                    let row = cursor
                        .next_edit(&mut work)?
                        .ok_or(IndexError::ResourceLimited)?;
                    check_eq!(row.root(), *ordinal);
                    check_eq!(row.key(), *key);
                    check_eq!(row.expected()?, None);
                    check_eq!(row.next()?, Some(MapValue::Object(*reference)));
                }
                check!(work.first_denial().is_none());
                check!(cursor.next_edit(&mut work)?.is_none());
            }
            Mode::WrongLast | Mode::Fail => {
                check!(result.is_err());
                check!(confirmed.references.is_empty());
                check!(spool.inventory_cursor(256)?.next_edit(&mut work).is_err());
                check!(!spool.confirmed_occurrence_contains(
                    *expected.first().ok_or(IndexError::ResourceLimited)?,
                    attempt,
                    &mut work
                )?);
            }
            Mode::Cancel => {
                check_eq!(result, Err(IndexError::Cancelled));
                check!(confirmed.references.is_empty());
                // A fresh budget demonstrates poison independently of the
                // cancelled request's checkpoint, without granting a retry.
                check!(
                    spool
                        .inventory_cursor(256)?
                        .next_edit(&mut packet_budget()?)
                        .is_err()
                );
            }
        }
        if limited {
            check!(denied.load(Ordering::Relaxed) >= 1);
        }
    }
    Ok(())
}
