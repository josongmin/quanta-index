//! Bounded producer buffering before physical pool installation.
//!
//! Prepared bytes are available only to this attempt's scoped builder. They
//! are not retained objects: the pool barrier and every original writer's
//! settlement must complete before an adoption head can name them.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::File;
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::PublicationOutputPermit;
use sha2::{Digest as _, Sha256};

use super::{
    AtomicSync, AttemptLease, BatchState, MAX_ADOPTION_BATCH_DATA_BYTES, MAX_ADOPTION_BATCH_REFS,
    ObjectContents, ObjectKind, ObjectRef, ObjectStore, PreparedObject, corrupt, decode_object,
    invalid, storage, with_custody_context,
};

#[derive(Default)]
pub(super) struct PreparedBatch {
    objects: Vec<PreparedObject>,
    bytes: u64,
    closed: bool,
}

/// Fixed-size, opt-in timing for the actual lexical pool install path.
/// One line is emitted per bounded prepared batch, including failed batches.
/// Parallel worker phase durations are summed and may exceed batch wall time.
#[derive(Clone, Copy)]
pub(super) enum InstallPhase {
    Decode,
    Custody,
    PoolReserve,
    Shard,
    Probe,
    OutputReserve,
    AtomicWrite,
    Account,
    Barrier,
    Settle,
    Retire,
}

impl InstallPhase {
    const ALL: [Self; 11] = [
        Self::Decode,
        Self::Custody,
        Self::PoolReserve,
        Self::Shard,
        Self::Probe,
        Self::OutputReserve,
        Self::AtomicWrite,
        Self::Account,
        Self::Barrier,
        Self::Settle,
        Self::Retire,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Decode => "decode",
            Self::Custody => "custody",
            Self::PoolReserve => "pool_reserve",
            Self::Shard => "shard",
            Self::Probe => "probe",
            Self::OutputReserve => "output_reserve",
            Self::AtomicWrite => "atomic_write",
            Self::Account => "account",
            Self::Barrier => "barrier",
            Self::Settle => "settle",
            Self::Retire => "retire",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Decode => 0,
            Self::Custody => 1,
            Self::PoolReserve => 2,
            Self::Shard => 3,
            Self::Probe => 4,
            Self::OutputReserve => 5,
            Self::AtomicWrite => 6,
            Self::Account => 7,
            Self::Barrier => 8,
            Self::Settle => 9,
            Self::Retire => 10,
        }
    }
}

/// Nested inside `InstallPhase::AtomicWrite`; never sum with its parent.
#[derive(Clone, Copy)]
pub(super) enum AtomicPhase {
    Preflight,
    BindOutput,
    CreationStart,
    TempOpen,
    WriterWitness,
    Write,
    FileSync,
    BeforeReplace,
    Rename,
    PoolCommit,
    ShardSync,
    Verify,
    Commit,
    Cleanup,
}

impl AtomicPhase {
    const ALL: [Self; 14] = [
        Self::Preflight,
        Self::BindOutput,
        Self::CreationStart,
        Self::TempOpen,
        Self::WriterWitness,
        Self::Write,
        Self::FileSync,
        Self::BeforeReplace,
        Self::Rename,
        Self::PoolCommit,
        Self::ShardSync,
        Self::Verify,
        Self::Commit,
        Self::Cleanup,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::BindOutput => "bind_output",
            Self::CreationStart => "creation_start",
            Self::TempOpen => "temp_open",
            Self::WriterWitness => "writer_witness",
            Self::Write => "write",
            Self::FileSync => "file_sync",
            Self::BeforeReplace => "before_replace",
            Self::Rename => "rename",
            Self::PoolCommit => "pool_commit",
            Self::ShardSync => "shard_sync",
            Self::Verify => "verify",
            Self::Commit => "commit",
            Self::Cleanup => "cleanup",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Preflight => 0,
            Self::BindOutput => 1,
            Self::CreationStart => 2,
            Self::TempOpen => 3,
            Self::WriterWitness => 4,
            Self::Write => 5,
            Self::FileSync => 6,
            Self::BeforeReplace => 7,
            Self::Rename => 8,
            Self::PoolCommit => 9,
            Self::ShardSync => 10,
            Self::Verify => 11,
            Self::Commit => 12,
            Self::Cleanup => 13,
        }
    }
}

#[derive(Clone, Copy)]
struct PhaseTiming {
    calls: u64,
    failures: u64,
    elapsed_ns: Option<u64>,
}

impl Default for PhaseTiming {
    fn default() -> Self {
        Self {
            calls: 0,
            failures: 0,
            elapsed_ns: Some(0),
        }
    }
}

pub(super) struct BatchInstallProfile {
    started: Option<Instant>,
    inputs: usize,
    workers: usize,
    phases: [PhaseTiming; InstallPhase::ALL.len()],
    atomic_phases: [PhaseTiming; AtomicPhase::ALL.len()],
    failed_object_kind: Option<ObjectKind>,
}

impl BatchInstallProfile {
    pub(super) fn new(inputs: usize) -> Self {
        Self {
            started: crate::causal_profile::enabled().then(Instant::now),
            inputs,
            workers: 1,
            phases: [PhaseTiming::default(); InstallPhase::ALL.len()],
            atomic_phases: [PhaseTiming::default(); AtomicPhase::ALL.len()],
            failed_object_kind: None,
        }
    }

    pub(super) fn disabled() -> Self {
        Self {
            started: None,
            inputs: 0,
            workers: 1,
            phases: [PhaseTiming::default(); InstallPhase::ALL.len()],
            atomic_phases: [PhaseTiming::default(); AtomicPhase::ALL.len()],
            failed_object_kind: None,
        }
    }

    pub(super) fn record_failed_object_kind(&mut self, kind: ObjectKind) {
        if self.started.is_some() {
            self.failed_object_kind = Some(kind);
        }
    }

    pub(super) fn set_workers(&mut self, workers: usize) {
        self.workers = workers;
    }

    pub(super) fn start(&self) -> Option<Instant> {
        self.started.map(|_| Instant::now())
    }

    fn record(cell: &mut PhaseTiming, started: Option<Instant>, failed: bool) {
        let Some(started) = started else {
            return;
        };
        cell.calls = cell.calls.saturating_add(1);
        cell.failures = cell.failures.saturating_add(u64::from(failed));
        cell.elapsed_ns = cell.elapsed_ns.and_then(|sum| {
            u64::try_from(started.elapsed().as_nanos())
                .ok()
                .and_then(|elapsed| sum.checked_add(elapsed))
        });
    }

    pub(super) fn record_atomic_total(&mut self, started: Option<Instant>, failed: bool) {
        Self::record(
            &mut self.phases[InstallPhase::AtomicWrite.index()],
            started,
            failed,
        );
    }

    pub(super) fn absorb(&mut self, other: Self) {
        for (target, source) in self.phases.iter_mut().zip(other.phases) {
            target.calls = target.calls.saturating_add(source.calls);
            target.failures = target.failures.saturating_add(source.failures);
            target.elapsed_ns = target
                .elapsed_ns
                .zip(source.elapsed_ns)
                .and_then(|(left, right)| left.checked_add(right));
        }
        for (target, source) in self.atomic_phases.iter_mut().zip(other.atomic_phases) {
            target.calls = target.calls.saturating_add(source.calls);
            target.failures = target.failures.saturating_add(source.failures);
            target.elapsed_ns = target
                .elapsed_ns
                .zip(source.elapsed_ns)
                .and_then(|(left, right)| left.checked_add(right));
        }
    }

    pub(super) fn measure<T, E>(
        &mut self,
        phase: InstallPhase,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let started = self.start();
        let result = operation();
        Self::record(&mut self.phases[phase.index()], started, result.is_err());
        result
    }

    pub(super) fn measure_atomic<T, E>(
        &mut self,
        phase: AtomicPhase,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let started = self.start();
        let result = operation();
        Self::record(
            &mut self.atomic_phases[phase.index()],
            started,
            result.is_err(),
        );
        result
    }

    #[expect(
        clippy::print_stderr,
        reason = "Opt-in source-bound lexical pool install diagnosis."
    )]
    pub(super) fn finish(&self, ok: bool) {
        let Some(started) = self.started else {
            return;
        };
        let mut line = String::new();
        if line.try_reserve(2048).is_err() {
            return;
        }
        let _written = write!(
            line,
            "QI_CAUSAL_V1 kind=lexical_object_install ok={} inputs={} workers={} total_elapsed_ns={}",
            u8::from(ok),
            self.inputs,
            self.workers,
            started.elapsed().as_nanos()
        );
        if let Some(kind) = self.failed_object_kind {
            let _written = write!(line, " failed_object_kind={kind:?}");
        }
        for (phase, cell) in InstallPhase::ALL.into_iter().zip(self.phases) {
            if cell.calls == 0 {
                continue;
            }
            let _written = write!(
                line,
                " {}_calls={} {}_failures={} {}_elapsed_ns=",
                phase.label(),
                cell.calls,
                phase.label(),
                cell.failures,
                phase.label()
            );
            if let Some(elapsed) = cell.elapsed_ns {
                let _written = write!(line, "{elapsed}");
            } else {
                line.push_str("unavailable");
            }
        }
        for (phase, cell) in AtomicPhase::ALL.into_iter().zip(self.atomic_phases) {
            if cell.calls == 0 {
                continue;
            }
            let _written = write!(
                line,
                " atomic_inner_{}_calls={} atomic_inner_{}_failures={} atomic_inner_{}_elapsed_ns=",
                phase.label(),
                cell.calls,
                phase.label(),
                cell.failures,
                phase.label()
            );
            if let Some(elapsed) = cell.elapsed_ns {
                let _written = write!(line, "{elapsed}");
            } else {
                line.push_str("unavailable");
            }
        }
        eprintln!("{line}");
    }
}

impl PreparedBatch {
    pub(super) fn close(&mut self) {
        self.closed = true;
        self.objects = Vec::new();
        self.bytes = 0;
    }
}

pub(super) struct PendingOutput {
    permit: Box<dyn PublicationOutputPermit>,
    writer: Option<File>,
    encoded_bytes: u64,
}

impl PendingOutput {
    pub(super) fn new(
        permit: Box<dyn PublicationOutputPermit>,
        writer: File,
        encoded_bytes: u64,
    ) -> Self {
        Self {
            permit,
            writer: Some(writer),
            encoded_bytes,
        }
    }

    fn commit(&mut self, store: &ObjectStore, expected: Option<&[u8]>) -> Result<(), CoreError> {
        let verification = (|| {
            let expected = expected
                .ok_or_else(|| invalid("pending writer lost its original encoded input"))?;
            store.verify_pool_writer(
                self.writer
                    .as_ref()
                    .ok_or_else(|| invalid("batch writer already retired"))?,
                expected,
            )
        })();
        if let Err(primary) = verification {
            return match self.retire() {
                Ok(()) => Err(primary),
                Err(retirement) => Err(with_custody_context(primary, retirement)),
            };
        }
        let writer = self
            .writer
            .take()
            .ok_or_else(|| invalid("batch writer already retired"))?;
        self.permit.committed(writer, self.encoded_bytes)
    }

    fn retire(&mut self) -> Result<(), CoreError> {
        if let Some(writer) = self.writer.take() {
            self.permit.retire_writer_preserving_pending(writer)?;
        }
        Ok(())
    }
}

impl Drop for PendingOutput {
    fn drop(&mut self) {
        // Normal errors collect retirement failures below. Unwind still
        // consumes the actual FD; the canonical owner quarantines close errors
        // and keeps uncertain capacity instead of treating Drop as proof.
        let _retirement = self.retire();
    }
}

impl ObjectStore {
    fn account_prepared_install(
        &self,
        lease: &AttemptLease,
        state: &mut BatchState,
        outputs: &mut VecDeque<(usize, PendingOutput)>,
        profile: &mut BatchInstallProfile,
        index: usize,
        installed: (ObjectRef, bool, Option<PendingOutput>),
    ) -> Result<(), CoreError> {
        let (reference, newly_written, pending) = installed;
        if let Some(pending) = pending {
            outputs.push_back((index, pending));
        }
        profile.measure(InstallPhase::Account, || {
            lease.record_write_cost(&reference, newly_written)?;
            if newly_written {
                state.encoded_new_bytes = state
                    .encoded_new_bytes
                    .checked_add(reference.bytes)
                    .ok_or_else(|| invalid("installed batch byte count overflow"))?;
            }
            Ok::<(), CoreError>(())
        })
    }

    fn account_prepared_outcome(
        &self,
        lease: &AttemptLease,
        state: &mut BatchState,
        outputs: &mut VecDeque<(usize, PendingOutput)>,
        profile: &mut BatchInstallProfile,
        index: usize,
        prepared: &PreparedObject,
        installed: Result<(ObjectRef, bool, Option<PendingOutput>), CoreError>,
    ) -> Result<(), CoreError> {
        match installed {
            Ok(installed) => {
                self.account_prepared_install(lease, state, outputs, profile, index, installed)
            }
            Err(error) => {
                profile.record_failed_object_kind(prepared.reference.kind);
                Err(error)
            }
        }
    }

    fn install_prepared_sequential(
        &self,
        objects: &[PreparedObject],
        lease: &AttemptLease,
        state: &mut BatchState,
        outputs: &mut VecDeque<(usize, PendingOutput)>,
        profile: &mut BatchInstallProfile,
    ) -> Result<(), CoreError> {
        for (index, prepared) in objects.iter().enumerate() {
            let (_, dependencies, _) =
                profile.measure(InstallPhase::Decode, || decode_object(&prepared.encoded))?;
            let installed = self.install_prepared_object(
                prepared,
                &dependencies,
                AtomicSync::BatchedPoolData,
                profile,
            );
            self.account_prepared_outcome(
                lease, state, outputs, profile, index, prepared, installed,
            )?;
        }
        Ok(())
    }

    pub(super) fn verify_pool_writer(&self, file: &File, expected: &[u8]) -> Result<(), CoreError> {
        if expected.len() > super::MAX_OBJECT_BYTES {
            return Err(invalid("pool writer verification exceeds the object bound"));
        }
        let metadata = file
            .metadata()
            .map_err(|error| storage("stat original pool writer", error))?;
        let expected_len =
            u64::try_from(expected.len()).map_err(|error| storage("pool encoded extent", error))?;
        if !metadata.is_file()
            || metadata.dev() != self.objects_device
            || metadata.len() != expected_len
        {
            return Err(corrupt("original pool writer differs from encoded extent"));
        }
        let mut offset = 0_usize;
        let mut buffer = [0_u8; 8192];
        let result = (|| {
            while offset < expected.len() {
                let remaining = expected
                    .len()
                    .checked_sub(offset)
                    .ok_or_else(|| invalid("pool verification offset exceeds input"))?;
                let width = buffer.len().min(remaining);
                let at =
                    u64::try_from(offset).map_err(|error| storage("pool read offset", error))?;
                let destination = buffer
                    .get_mut(..width)
                    .ok_or_else(|| invalid("pool verification buffer width exceeds bound"))?;
                let read = file
                    .read_at(destination, at)
                    .map_err(|error| storage("read original pool writer", error))?;
                if read == 0 {
                    return Err(corrupt("original pool writer ended before encoded extent"));
                }
                let begin = offset;
                offset = offset
                    .checked_add(read)
                    .ok_or_else(|| invalid("pool verification offset overflow"))?;
                let actual = buffer
                    .get(..read)
                    .ok_or_else(|| invalid("pool verification read exceeds buffer"))?;
                let original = expected
                    .get(begin..offset)
                    .ok_or_else(|| corrupt("original pool writer exceeds encoded extent"))?;
                if actual != original {
                    return Err(corrupt("original pool writer differs from encoded bytes"));
                }
            }
            if file
                .metadata()
                .map_err(|error| storage("recheck original pool extent", error))?
                .len()
                != expected_len
            {
                return Err(corrupt(
                    "original pool writer extent changed during verification",
                ));
            }
            Ok(())
        })();
        let metering = (|| {
            if let Some(meter) = &self.read_meter {
                let bytes =
                    u64::try_from(offset).map_err(|error| storage("pool read width", error))?;
                let _prior = meter
                    .bytes
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                        n.checked_add(bytes)
                    })
                    .map_err(|_overflow| invalid("scrub object byte meter overflow"))?;
                let _prior = meter
                    .objects
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                    .map_err(|_overflow| invalid("scrub object read meter overflow"))?;
            }
            Ok(())
        })();
        match (result, metering) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
            (Err(primary), Err(secondary)) => Err(with_custody_context(primary, secondary)),
        }
    }

    pub(super) fn verify_pool_writer_name(
        &self,
        parent: &File,
        name: &str,
        file: &File,
    ) -> Result<(), CoreError> {
        let current = rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| match error {
                rustix::io::Errno::NOENT | rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR => {
                    corrupt("published pool name is missing or not a regular file")
                }
                other => storage("stat published pool writer", other),
            })?;
        let original = file
            .metadata()
            .map_err(|error| storage("stat original pool identity", error))?;
        if rustix::fs::FileType::from_raw_mode(current.st_mode) != rustix::fs::FileType::RegularFile
            || i128::from(current.st_dev) != i128::from(original.dev())
            || i128::from(current.st_ino) != i128::from(original.ino())
        {
            return Err(corrupt("published pool name differs from original writer"));
        }
        Ok(())
    }
    pub(super) fn require_prepared_batch_open(&self) -> Result<(), CoreError> {
        self.prepared_batch_bytes().map(|_| ())
    }
    pub(super) fn prepared_batch_bytes(&self) -> Result<Option<u64>, CoreError> {
        self.prepared_batch
            .as_ref()
            .map(|batch| {
                let batch = batch
                    .lock()
                    .map_err(|error| storage("lock prepared batch", error))?;
                if batch.closed {
                    return Err(invalid("prepared batch view is closed"));
                }
                Ok(batch.bytes)
            })
            .transpose()
    }

    pub(super) fn stage_prepared_object(
        &self,
        prepared: PreparedObject,
    ) -> Result<(ObjectRef, bool), CoreError> {
        let batch = self
            .prepared_batch
            .as_ref()
            .ok_or_else(|| invalid("prepared batch is absent"))?;
        let mut batch = batch
            .lock()
            .map_err(|error| storage("lock prepared batch", error))?;
        if batch.closed {
            return Err(invalid("prepared batch view is closed"));
        }
        let reference = prepared.reference.clone();
        if batch
            .objects
            .iter()
            .any(|object| object.reference == reference)
        {
            return Ok((reference, false));
        }
        let bytes = batch
            .bytes
            .checked_add(reference.bytes)
            .ok_or_else(|| invalid("prepared batch byte count overflow"))?;
        if batch.objects.len() >= MAX_ADOPTION_BATCH_REFS || bytes > MAX_ADOPTION_BATCH_DATA_BYTES {
            return Err(invalid("prepared objects exceed the adoption batch bounds"));
        }
        batch
            .objects
            .try_reserve(1)
            .map_err(|error| storage("reserve prepared object", error))?;
        batch.objects.push(prepared);
        batch.bytes = bytes;
        drop(batch);
        Ok((reference, true))
    }

    pub(super) fn read_prepared_object(
        &self,
        reference: &ObjectRef,
    ) -> Result<Option<ObjectContents>, CoreError> {
        let Some(batch) = &self.prepared_batch else {
            return Ok(None);
        };
        let batch = batch
            .lock()
            .map_err(|error| storage("lock prepared reader", error))?;
        if batch.closed {
            return Err(invalid("prepared batch view is closed"));
        }
        let Some(prepared) = batch
            .objects
            .iter()
            .find(|object| object.reference.sha256 == reference.sha256)
        else {
            return Ok(None);
        };
        let digest: [u8; 32] = Sha256::digest(&prepared.encoded).into();
        if &prepared.reference != reference || digest != reference.sha256 {
            return Err(corrupt(
                "prepared object differs from its original identity",
            ));
        }
        let (kind, dependencies, payload) = decode_object(&prepared.encoded)?;
        drop(batch);
        if kind != reference.kind {
            return Err(corrupt("prepared object kind differs"));
        }
        Ok(Some((dependencies, payload)))
    }

    fn settle_or_retire_failed_prefix(
        &self,
        outputs: &mut VecDeque<(usize, PendingOutput)>,
        prepared: &[PreparedObject],
        profile: &mut BatchInstallProfile,
        barrier_attempted: bool,
        mut barrier_succeeded: bool,
        mut primary: CoreError,
    ) -> CoreError {
        // A later installation can refuse its allowance after earlier
        // immutable names were linked. The known prefix has already had its
        // file and shard POSIX syncs. Complete the shared barrier and settle
        // original writers before discarding their exact prepared bytes.
        if !barrier_attempted && !outputs.is_empty() {
            match profile.measure(InstallPhase::Barrier, || self.sync_pool_before_adoption()) {
                Ok(()) => barrier_succeeded = true,
                Err(barrier) => primary = with_custody_context(primary, barrier),
            }
        }
        if barrier_succeeded {
            while let Some((index, mut output)) = outputs.pop_front() {
                if let Err(settlement) = profile.measure(InstallPhase::Settle, || {
                    output.commit(
                        self,
                        prepared.get(index).map(|object| object.encoded.as_slice()),
                    )
                }) {
                    primary = with_custody_context(primary, settlement);
                }
            }
        } else {
            // A refused barrier cannot grant settlement. Retire only the
            // actual FDs and preserve the linked outputs' pending custody.
            for (_, output) in outputs {
                if let Err(retirement) = profile.measure(InstallPhase::Retire, || output.retire()) {
                    primary = with_custody_context(primary, retirement);
                }
            }
        }
        primary
    }

    pub(super) fn install_prepared_batch(&self, state: &mut BatchState) -> Result<bool, CoreError> {
        let Some(batch) = &self.prepared_batch else {
            return Ok(false);
        };
        let mut batch = batch
            .lock()
            .map_err(|error| storage("lock prepared installation", error))?;
        if batch.closed {
            return Err(invalid("prepared batch view is closed"));
        }
        if batch.objects.is_empty() {
            return Ok(false);
        }
        let mut outputs = VecDeque::new();
        outputs
            .try_reserve_exact(batch.objects.len())
            .map_err(|error| storage("reserve pending batch writers", error))?;
        let lease = AttemptLease {
            store: self.clone(),
            name: state.lease_name.clone(),
            binding: state.binding.clone(),
            write_cost: Arc::clone(&state.write_cost),
        };
        let mut profile = BatchInstallProfile::new(batch.objects.len());
        let mut barrier_attempted = false;
        let mut barrier_succeeded = false;
        let result = (|| {
            // Small groups keep the original sequential installation path.
            // Larger groups overlap only independent physical object writes;
            // quota reconciliation and admission remain ordered under the gate.
            // The opt-in profiler can compare the existing serial path with
            // the default pair writer on the same prepared batch size.
            let diagnostic_serial = crate::causal_profile::enabled()
                && std::env::var_os("QUANTA_INDEX_CAUSAL_INSTALL_SERIAL_V1")
                    .is_some_and(|value| value == std::ffi::OsStr::new("1"));
            if batch.objects.len() < 16 || diagnostic_serial {
                self.install_prepared_sequential(
                    &batch.objects,
                    &lease,
                    state,
                    &mut outputs,
                    &mut profile,
                )?;
            } else {
                std::thread::scope(|scope| {
                    let objects = batch.objects.as_slice();
                    let worker_objects = objects;
                    let (jobs, pending) = std::sync::mpsc::sync_channel(1);
                    let (completed, results) = std::sync::mpsc::sync_channel(1);
                    let worker = std::thread::Builder::new()
                        .name("lexical-pool-install".into())
                        .spawn_scoped(scope, move || {
                            while let Ok((index, plan)) = pending.recv() {
                                let mut local = BatchInstallProfile::new(0);
                                let installed = self.finish_prepared_object_install(
                                    &worker_objects[index],
                                    AtomicSync::BatchedPoolData,
                                    plan,
                                    &mut local,
                                );
                                if completed.send((index, installed, local)).is_err() {
                                    break;
                                }
                            }
                        });
                    let worker = match worker {
                        Ok(worker) => worker,
                        Err(_spawn) => {
                            return self.install_prepared_sequential(
                                objects,
                                &lease,
                                state,
                                &mut outputs,
                                &mut profile,
                            );
                        }
                    };
                    profile.set_workers(2);
                    let installed = (|| -> Result<(), CoreError> {
                        for (pair, chunk) in objects.chunks(2).enumerate() {
                            let offset = pair
                                .checked_mul(2)
                                .ok_or_else(|| invalid("prepared pair offset overflow"))?;
                            let mut plans = Vec::new();
                            plans
                                .try_reserve_exact(chunk.len())
                                .map_err(|error| storage("reserve prepared install pair", error))?;
                            for (relative, prepared) in chunk.iter().enumerate() {
                                let (_, dependencies, _) = profile
                                    .measure(InstallPhase::Decode, || {
                                        decode_object(&prepared.encoded)
                                    })?;
                                let plan = self.prepare_prepared_object_install(
                                    prepared,
                                    &dependencies,
                                    &mut profile,
                                );
                                if plan.is_err() {
                                    profile.record_failed_object_kind(prepared.reference.kind);
                                }
                                plans.push((offset + relative, plan?));
                            }
                            let (first_index, first_plan) = plans.remove(0);
                            jobs.send((first_index, first_plan))
                                .map_err(|_closed| invalid("prepared install worker closed"))?;
                            let owner_result = plans.pop().map(|(index, plan)| {
                                let installed = self.finish_prepared_object_install(
                                    &objects[index],
                                    AtomicSync::BatchedPoolData,
                                    plan,
                                    &mut profile,
                                );
                                (index, installed)
                            });
                            let first = match results.recv() {
                                Ok((worker_index, worker_result, local)) => {
                                    profile.absorb(local);
                                    self.account_prepared_outcome(
                                        &lease,
                                        state,
                                        &mut outputs,
                                        &mut profile,
                                        worker_index,
                                        &objects[worker_index],
                                        worker_result,
                                    )
                                }
                                Err(_closed) => Err(invalid("prepared install worker lost result")),
                            };
                            let second = owner_result.map(|(index, installed)| {
                                self.account_prepared_outcome(
                                    &lease,
                                    state,
                                    &mut outputs,
                                    &mut profile,
                                    index,
                                    &objects[index],
                                    installed,
                                )
                            });
                            match (first, second) {
                                (Err(primary), Some(Err(secondary))) => {
                                    return Err(with_custody_context(primary, secondary));
                                }
                                (Err(error), _) | (Ok(()), Some(Err(error))) => {
                                    return Err(error);
                                }
                                (Ok(()), Some(Ok(())) | None) => {}
                            }
                        }
                        Ok(())
                    })();
                    drop(jobs);
                    let joined = worker
                        .join()
                        .map_err(|_panic| invalid("prepared install worker panicked"));
                    match (installed, joined) {
                        (Ok(()), Ok(())) => Ok(()),
                        (Err(primary), Err(secondary)) => {
                            Err(with_custody_context(primary, secondary))
                        }
                        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
                    }
                })?;
            }
            // POSIX file/shard syncs precede this drive barrier. No output
            // is settled and no adoption head is written before it succeeds.
            barrier_attempted = true;
            profile.measure(InstallPhase::Barrier, || self.sync_pool_before_adoption())?;
            barrier_succeeded = true;
            while let Some((index, mut output)) = outputs.pop_front() {
                profile.measure(InstallPhase::Settle, || {
                    output.commit(
                        self,
                        batch
                            .objects
                            .get(index)
                            .map(|object| object.encoded.as_slice()),
                    )
                })?;
                // Linked immutable output remains charged after the permit
                // drops. Release its settled reader pin before the next one.
            }
            Ok(())
        })();
        if let Err(primary) = result {
            // No AttemptBatch or adoption head is written on this path.
            let primary = self.settle_or_retire_failed_prefix(
                &mut outputs,
                &batch.objects,
                &mut profile,
                barrier_attempted,
                barrier_succeeded,
                primary,
            );
            batch.close();
            profile.finish(false);
            return Err(primary);
        }
        batch.objects.clear();
        batch.bytes = 0;
        drop(batch);
        profile.finish(true);
        // Every new object was read on its original FD after the barrier and
        // authenticated by its output permit before reaching the journal.
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AttemptBinding, BatchScope, OBJECTS_DIR, ObjectKind, POOL_DIR, hex_digest};
    use super::*;
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE;

    const GOLD: &[u8; 24] = b"QLSTOR01\x00\x01\x07\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03abc";

    #[test]
    fn original_writer_verification_detects_same_width_corruption_and_meters_actual_reads() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path())
            .expect("store")
            .metered_clone();
        let mut file = tempfile::tempfile_in(dir.path()).expect("writer");
        std::io::Write::write_all(&mut file, GOLD).expect("write");
        store
            .verify_pool_writer(&file, GOLD)
            .expect("exact persisted bytes");
        assert_eq!(store.read_meter_snapshot(), (24, 1));
        std::os::unix::fs::FileExt::write_all_at(&file, b"X", 23).expect("same-width corruption");
        assert!(matches!(
            store.verify_pool_writer(&file, GOLD),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        assert_eq!(store.read_meter_snapshot(), (48, 2));
    }

    #[test]
    fn original_writer_verification_rejects_a_foreign_published_name() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let target = dir.path().join("original");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&target)
            .expect("writer");
        std::io::Write::write_all(&mut file, GOLD).expect("write");
        let parent = File::open(dir.path()).expect("parent");
        store
            .verify_pool_writer_name(&parent, "original", &file)
            .expect("original name");
        std::fs::rename(&target, dir.path().join("saved")).expect("move original");
        std::fs::write(&target, GOLD).expect("same bytes on a foreign inode");
        assert!(matches!(
            store.verify_pool_writer_name(&parent, "original", &file),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
        std::fs::remove_file(&target).expect("remove foreign name");
        assert!(matches!(
            store.verify_pool_writer_name(&parent, "original", &file),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }

    fn buffered(store: &ObjectStore, lease: &AttemptLease) -> ObjectStore {
        let mut scoped = store.clone();
        scoped.prepared_batch = Some(Arc::new(Mutex::new(PreparedBatch::default())));
        scoped.adoption_batch = Some(Arc::new(Mutex::new(BatchState {
            lease_name: lease.name.clone(),
            binding: lease.binding.clone(),
            write_cost: Arc::clone(&lease.write_cost),
            gate: None,
            references: Vec::new(),
            encoded_new_bytes: 0,
            closed: false,
        })));
        scoped
    }

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [5; 32],
            format: 1,
        }
    }

    struct FailedPrefixPermit {
        target: PathBuf,
        original_fd: i32,
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_commit: bool,
    }

    impl FailedPrefixPermit {
        fn unexpected() -> CoreError {
            invalid("failed-prefix fixture received an unrelated output transition")
        }

        fn event(&self, event: &'static str) {
            self.events.lock().expect("fixture events").push(event);
        }
    }

    impl PublicationOutputPermit for FailedPrefixPermit {
        fn bind_output(&mut self, _staging: &Path, _target: &Path) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn creation_started(&mut self) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn created(&mut self, _writer: &File, _staging: &Path) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn before_replace(&mut self) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
            assert_eq!(writer.as_raw_fd(), self.original_fd);
            let original = writer.metadata().expect("original writer metadata");
            let current = std::fs::metadata(&self.target).expect("published name metadata");
            assert_eq!(
                (original.dev(), original.ino()),
                (current.dev(), current.ino())
            );
            assert_eq!(original.len(), actual_encoded_bytes);
            drop(writer);
            if self.fail_commit {
                self.event("commit_failed");
                return Err(storage("fixture settlement", "injected failure"));
            }
            self.event("committed");
            Ok(())
        }

        fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError> {
            assert_eq!(writer.as_raw_fd(), self.original_fd);
            drop(writer);
            self.event("retired_pending");
            Ok(())
        }

        fn cancel_rejected_created(&mut self, writer: File) -> Result<(), CoreError> {
            drop(writer);
            Err(Self::unexpected())
        }

        fn rename_started(&mut self, _target: &Path) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
            Err(Self::unexpected())
        }

        fn confirmed_safe_absent_after_writer_close(
            &mut self,
            writer: File,
        ) -> Result<(), CoreError> {
            drop(writer);
            Err(Self::unexpected())
        }
    }

    fn failed_prefix(
        store: &ObjectStore,
        fail_commit: bool,
    ) -> (
        Vec<PreparedObject>,
        VecDeque<(usize, PendingOutput)>,
        Arc<Mutex<Vec<&'static str>>>,
        PathBuf,
    ) {
        let prepared = PreparedObject::new(ObjectKind::Record, &[], b"prefix")
            .expect("bounded prepared object");
        let shard = store
            .object_dir(&prepared.reference.sha256, true)
            .expect("physical object shard");
        let digest = hex_digest(&prepared.reference.sha256);
        let target = store
            .state_root
            .join(POOL_DIR)
            .join(OBJECTS_DIR)
            .join(digest.get(..2).expect("digest shard"))
            .join(digest);
        let mut writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&target)
            .expect("published prefix writer");
        writer.write_all(&prepared.encoded).expect("prefix bytes");
        writer.sync_all().expect("prefix file sync");
        shard.sync_all().expect("prefix shard sync");
        let events = Arc::new(Mutex::new(Vec::new()));
        let permit = FailedPrefixPermit {
            target: target.clone(),
            original_fd: writer.as_raw_fd(),
            events: Arc::clone(&events),
            fail_commit,
        };
        let mut outputs = VecDeque::new();
        outputs.push_back((
            0,
            PendingOutput::new(Box::new(permit), writer, prepared.reference.bytes),
        ));
        (vec![prepared], outputs, events, target)
    }

    fn later_budget_refusal() -> CoreError {
        CoreError::Typed {
            code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
            message: "later output exceeds held publication allowance".into(),
        }
    }

    #[test]
    fn failed_prepared_prefix_settles_original_writer_without_adoption() {
        let directory = tempfile::tempdir().expect("temporary pool");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let active = binding();
        let _lease = store.begin_attempt(&active, &[]).expect("attempt");
        let (prepared, mut outputs, events, target) = failed_prefix(&store, false);
        let result = store.settle_or_retire_failed_prefix(
            &mut outputs,
            &prepared,
            &mut BatchInstallProfile::disabled(),
            false,
            false,
            later_budget_refusal(),
        );
        assert!(matches!(result, CoreError::Typed { code, .. }
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE));
        assert!(outputs.is_empty());
        assert_eq!(*events.lock().expect("events"), ["committed"]);
        assert_eq!(
            std::fs::read(target).expect("linked prefix"),
            prepared[0].encoded
        );
        assert_eq!(
            store
                .read_journal_head(&active.identity_name())
                .expect("journal head")
                .committed_length,
            0
        );
        assert!(store.adopted_roots().expect("adopted roots").is_empty());
    }

    #[test]
    fn failed_prepared_prefix_retains_pending_when_barrier_fails() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        let directory = tempfile::tempdir().expect("temporary pool");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let (prepared, mut outputs, events, target) = failed_prefix(&store, false);
        let (result, fired) = with_error_at(Cut::PoolFullBarrier, Side::Before, 1, || {
            store.settle_or_retire_failed_prefix(
                &mut outputs,
                &prepared,
                &mut BatchInstallProfile::disabled(),
                false,
                false,
                later_budget_refusal(),
            )
        });
        assert!(fired);
        assert!(matches!(result, CoreError::Typed { code, message }
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
                && message.contains("physical custody cleanup failed")));
        assert_eq!(*events.lock().expect("events"), ["retired_pending"]);
        assert_eq!(
            std::fs::read(target).expect("linked pending"),
            prepared[0].encoded
        );
    }

    #[test]
    fn failed_prepared_prefix_appends_settlement_refusal_to_original_error() {
        let directory = tempfile::tempdir().expect("temporary pool");
        let store = ObjectStore::open(directory.path()).expect("object store");
        let (prepared, mut outputs, events, target) = failed_prefix(&store, true);
        let result = store.settle_or_retire_failed_prefix(
            &mut outputs,
            &prepared,
            &mut BatchInstallProfile::disabled(),
            false,
            false,
            later_budget_refusal(),
        );
        assert!(matches!(result, CoreError::Typed { code, message }
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
                && message.contains("injected failure")));
        assert_eq!(*events.lock().expect("events"), ["commit_failed"]);
        assert_eq!(
            std::fs::read(target).expect("linked unsettled"),
            prepared[0].encoded
        );
    }

    #[test]
    fn prepared_bytes_are_private_until_barrier_and_adoption() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let _scope = BatchScope { store: &scoped };
        let reference = scoped
            .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
            .expect("prepare object");
        assert_eq!(reference.bytes, 24);
        let expected_digest: [u8; 32] = Sha256::digest(GOLD).into();
        assert_eq!(reference.sha256, expected_digest);
        assert_eq!(
            scoped
                .read_object(&reference, None)
                .expect("producer read")
                .1,
            b"abc"
        );
        assert!(
            store.read_object(&reference, None).is_err(),
            "unscoped read must not see staging"
        );
        assert!(store.adopted_roots().expect("roots").is_empty());
        assert_eq!(lease.write_cost_snapshot().expect("cost").new_objects, 0);
        scoped
            .adopt_existing_root(&lease, &reference)
            .expect("private dependency");
        scoped.flush_adoption_batch().expect("install and adopt");
        assert!(
            store
                .adopted_roots()
                .expect("durable roots")
                .contains(&reference)
        );
        let digest = hex_digest(&reference.sha256);
        let path = dir
            .path()
            .join(POOL_DIR)
            .join(OBJECTS_DIR)
            .join(digest.get(..2).expect("shard"))
            .join(digest);
        assert_eq!(std::fs::read(path).expect("physical bytes"), GOLD);
        assert_eq!(scoped.prepared_batch_bytes().expect("buffer"), Some(0));
    }

    #[test]
    fn failed_prepared_barrier_never_advances_head_or_leaves_a_readable_scope() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let active = binding();
        let lease = store.begin_attempt(&active, &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let scope = BatchScope { store: &scoped };
        let reference = scoped
            .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
            .expect("prepare object");
        let (failed, fired) = with_error_at(Cut::PoolFullBarrier, Side::Before, 1, || {
            scoped.flush_adoption_batch()
        });
        assert!(fired);
        assert!(failed.is_err());
        assert_eq!(
            store
                .read_journal_head(&active.identity_name())
                .expect("head")
                .committed_length,
            0
        );
        assert!(store.adopted_roots().expect("roots").is_empty());
        assert!(matches!(
            scoped.read_object(&reference, None),
            Err(CoreError::InvalidContract(_))
        ));
        drop(scope);
        drop(scoped);
        drop(lease);
        let reopened = ObjectStore::open(dir.path()).expect("cold open");
        assert!(reopened.adopted_roots().expect("cold roots").is_empty());
        let retry = reopened.begin_attempt(&active, &[]).expect("retry");
        let retried = reopened
            .put_adopted_object(&retry, ObjectKind::Record, &[], b"abc")
            .expect("repair and adopt");
        assert_eq!(retried, reference);
    }

    #[test]
    fn parallel_prepared_install_joins_before_barrier_and_adoption() {
        use crate::publication_faults::{Cut, Side, with_error_at};

        for fail_barrier in [false, true] {
            let dir = tempfile::tempdir().expect("root");
            let store = ObjectStore::open(dir.path()).expect("store");
            let active = binding();
            let lease = store.begin_attempt(&active, &[]).expect("attempt");
            let scoped = buffered(&store, &lease);
            let scope = BatchScope { store: &scoped };
            let mut references = Vec::new();
            for value in 0..18_u16 {
                references.push(
                    scoped
                        .put_adopted_object(&lease, ObjectKind::Record, &[], &value.to_be_bytes())
                        .expect("prepare object"),
                );
            }
            assert!(store.adopted_roots().expect("private roots").is_empty());
            if fail_barrier {
                let (result, fired) = with_error_at(Cut::PoolFullBarrier, Side::Before, 1, || {
                    scoped.flush_adoption_batch()
                });
                assert!(fired);
                assert!(result.is_err());
                assert!(store.adopted_roots().expect("failed roots").is_empty());
                assert_eq!(
                    store
                        .read_journal_head(&active.identity_name())
                        .expect("failed head")
                        .committed_length,
                    0
                );
            } else {
                scoped.flush_adoption_batch().expect("install and adopt");
                let roots = store.adopted_roots().expect("adopted roots");
                assert!(references.iter().all(|reference| roots.contains(reference)));
            }
            drop(scope);
            drop(scoped);
            drop(lease);
            drop(store);
            let cold = ObjectStore::open(dir.path()).expect("cold reopen");
            let roots = cold.adopted_roots().expect("cold roots");
            if fail_barrier {
                assert!(roots.is_empty());
            } else {
                assert!(references.iter().all(|reference| roots.contains(reference)));
            }
        }
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn new_shard_sync_failure_never_adopts_and_cold_retry_repairs_parent_metadata() {
        use crate::publication_faults::{Cut, Side, with_error_at};
        for cut in [Cut::PoolNewDirectorySyncCall, Cut::PoolNewParentSyncCall] {
            let dir = tempfile::tempdir().expect("root");
            let store = ObjectStore::open(dir.path()).expect("store");
            let active = binding();
            let lease = store.begin_attempt(&active, &[]).expect("attempt");
            let scoped = buffered(&store, &lease);
            let scope = BatchScope { store: &scoped };
            let reference = scoped
                .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
                .expect("prepare");
            let (failed, fired) =
                with_error_at(cut, Side::Before, 1, || scoped.flush_adoption_batch());
            assert!(fired);
            assert!(failed.is_err());
            assert_eq!(
                lease.write_cost_snapshot().expect("no birth").new_objects,
                0
            );
            assert_eq!(
                store
                    .read_journal_head(&active.identity_name())
                    .expect("head")
                    .committed_length,
                0
            );
            assert!(store.adopted_roots().expect("roots").is_empty());
            drop(scope);
            drop(scoped);
            drop(lease);
            drop(store);
            let cold = ObjectStore::open(dir.path()).expect("cold store");
            let retry = cold.begin_attempt(&active, &[]).expect("retry");
            let next = buffered(&cold, &retry);
            let scope = BatchScope { store: &next };
            let retried = next
                .put_adopted_object(&retry, ObjectKind::Record, &[], b"abc")
                .expect("prepare retry");
            assert_eq!(reference, retried);
            // An existing shard from the failed mkdir path must still cross
            // its parent-sync boundary before any new file can be adopted.
            let (failed, fired) =
                with_error_at(Cut::PoolNewParentSyncCall, Side::Before, 1, || {
                    next.flush_adoption_batch()
                });
            assert!(fired);
            assert!(failed.is_err());
            assert_eq!(
                retry
                    .write_cost_snapshot()
                    .expect("still no birth")
                    .new_objects,
                0
            );
            drop(scope);
            drop(next);
            let committed = cold
                .put_adopted_object(&retry, ObjectKind::Record, &[], b"abc")
                .expect("repair and commit");
            assert_eq!(reference, committed);
            assert_eq!(
                cold.read_object(&committed, None).expect("stored").1,
                b"abc"
            );
        }
    }

    #[test]
    fn prepared_count_and_byte_caps_precede_physical_creation() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let _scope = BatchScope { store: &scoped };
        for value in 0..128_u16 {
            let _object = scoped
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], &value.to_be_bytes())
                        .expect("object"),
                )
                .expect("bounded preparation");
        }
        assert!(
            scoped
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], b"overflow").expect("object")
                )
                .is_err()
        );
        assert!(store.adopted_roots().expect("roots").is_empty());
        assert_eq!(lease.write_cost_snapshot().expect("cost").new_objects, 0);
        let byte_limited = buffered(&store, &lease);
        let _byte_scope = BatchScope {
            store: &byte_limited,
        };
        for value in 0..31_u8 {
            let payload = vec![value; 65_536];
            let _object = byte_limited
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], &payload).expect("object"),
                )
                .expect("bounded bytes");
        }
        assert!(
            byte_limited
                .stage_prepared_object(
                    PreparedObject::new(ObjectKind::Record, &[], &vec![31; 65_536])
                        .expect("object")
                )
                .is_err()
        );
        assert_eq!(
            byte_limited.prepared_batch_bytes().expect("bytes"),
            Some(2_032_267)
        );
    }

    #[test]
    fn escaped_producer_cannot_read_after_scope_retirement() {
        let dir = tempfile::tempdir().expect("root");
        let store = ObjectStore::open(dir.path()).expect("store");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let scoped = buffered(&store, &lease);
        let scope = BatchScope { store: &scoped };
        let escaped = scoped.metered_clone();
        let reference = escaped
            .put_adopted_object(&lease, ObjectKind::Record, &[], b"abc")
            .expect("prepare");
        let _read = escaped
            .read_object(&reference, None)
            .expect("producer read");
        scoped.flush_adoption_batch().expect("install");
        drop(scope);
        assert!(store.read_object(&reference, None).is_ok());
        assert!(matches!(
            escaped.read_object(&reference, None),
            Err(CoreError::InvalidContract(_))
        ));
    }
}
