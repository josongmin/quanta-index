//! Check child span balance, counters and actual caller evidence in its own clock.

use anyhow::{Context as _, Result};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, Metadata, OpenOptions},
    io::{BufReader, Read as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

#[cfg(test)]
pub(super) const MAX_BYTES: u64 = 67_108_864;
const MAX_LINE_BYTES: usize = 4096;

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileEpoch {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    owner: u32,
    group: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanos: i64,
    changed_seconds: i64,
    changed_nanos: i64,
}

impl FileEpoch {
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            group: metadata.gid(),
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
        }
    }
}

fn read_bounded_line(reader: &mut impl std::io::BufRead, line: &mut Vec<u8>) -> Result<usize> {
    line.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(line.len());
        }
        let end = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index.saturating_add(1));
        anyhow::ensure!(
            end <= MAX_LINE_BYTES.saturating_sub(line.len()),
            "invalid child capture record boundary"
        );
        line.try_reserve(end)?;
        line.extend_from_slice(
            available
                .get(..end)
                .context("child capture line exceeded buffered input")?,
        );
        reader.consume(end);
        if line.last() == Some(&b'\n') {
            return Ok(line.len());
        }
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Evidence {
    Boot,
    Request,
    Build,
    Publish,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum SyncCounter {
    Calls,
    Failures,
    ElapsedNs,
}

#[derive(Default)]
struct Spans {
    count: u64,
    open: BTreeSet<u64>,
    seen: BTreeSet<u64>,
    parents: BTreeMap<u64, Option<u64>>,
    witnesses: BTreeMap<u64, Evidence>,
    request_roots: BTreeSet<u64>,
    identified_requests: BTreeSet<u64>,
    evidence: BTreeSet<Evidence>,
    operation_roots: BTreeSet<u64>,
    boot_roots: BTreeSet<u64>,
    sync: BTreeMap<u64, BTreeMap<SyncCounter, u64>>,
}

impl Spans {
    fn operation_ancestor(&self, mut span: u64) -> Option<u64> {
        loop {
            if self.operation_roots.contains(&span) {
                return Some(span);
            }
            span = self.parents.get(&span).copied().flatten()?;
        }
    }

    // IPC Request roots are independent of the daemon's Operation root.
    // Caller evidence belongs under a root with an emitted request identity.
    fn beneath_identified_request_root(&self, mut span: u64) -> bool {
        loop {
            if self.identified_requests.contains(&span) {
                return true;
            }
            let Some(Some(parent)) = self.parents.get(&span) else {
                return false;
            };
            span = *parent;
        }
    }

    fn event(&mut self, row: &Value) -> Result<()> {
        self.count = self
            .count
            .checked_add(1)
            .context("child event count overflow")?;
        anyhow::ensure!(row.get("output_seq").and_then(Value::as_u64) == Some(self.count));
        let kind = row.get("kind").context("child event kind absent")?;
        match kind.get("type").and_then(Value::as_str) {
            Some("span_begin") => {
                let span = kind
                    .get("span_id")
                    .and_then(Value::as_u64)
                    .context("child span ID absent")?;
                anyhow::ensure!(
                    span > 0 && self.seen.insert(span) && self.open.insert(span),
                    "duplicate child span"
                );
                let parent = kind.get("parent_span_id").and_then(Value::as_u64);
                if let Some(parent) = parent {
                    anyhow::ensure!(
                        self.parents.contains_key(&parent),
                        "child span parent absent or self-referential"
                    );
                }
                let _previous = self.parents.insert(span, parent);
                if kind.get("stage").and_then(Value::as_str) == Some("request")
                    && kind.get("root_kind").and_then(Value::as_str) == Some("request")
                    && parent.is_none()
                {
                    let _new = self.request_roots.insert(span);
                }
                if kind.get("stage").and_then(Value::as_str) == Some("operation")
                    && parent.is_none()
                {
                    let _new = self.operation_roots.insert(span);
                }
                match kind.get("stage").and_then(Value::as_str) {
                    Some("runtime_boot") => {
                        let _new = self.evidence.insert(Evidence::Boot);
                        if let Some(root) =
                            parent.and_then(|parent| self.operation_ancestor(parent))
                        {
                            let _new = self.boot_roots.insert(root);
                        }
                    }
                    Some("build_prepare") => {
                        let _previous = self.witnesses.insert(span, Evidence::Build);
                    }
                    Some("native_publish") => {
                        let _previous = self.witnesses.insert(span, Evidence::Publish);
                    }
                    _ => {}
                }
            }
            Some("span_end") => {
                let span = kind
                    .get("span_id")
                    .and_then(Value::as_u64)
                    .context("child end ID absent")?;
                anyhow::ensure!(self.open.remove(&span), "unmatched child span end");
                if kind.get("outcome").and_then(Value::as_str) == Some("ok")
                    && self.beneath_identified_request_root(span)
                    && let Some(witness) = self.witnesses.get(&span)
                {
                    let _new = self.evidence.insert(*witness);
                }
            }
            Some("request_identity") => {
                let span = kind
                    .get("span_id")
                    .and_then(Value::as_u64)
                    .context("child request span absent")?;
                anyhow::ensure!(
                    self.open.contains(&span) && self.request_roots.contains(&span),
                    "child request identity outside request root"
                );
                let _new = self.identified_requests.insert(span);
                let _new = self.evidence.insert(Evidence::Request);
            }
            Some("counter") => self.counter(kind)?,
            _ => {}
        }
        Ok(())
    }

    fn counter(&mut self, kind: &Value) -> Result<()> {
        let Some(root) = kind.get("span_id").and_then(Value::as_u64) else {
            return Ok(());
        };
        if !self.operation_roots.contains(&root) {
            return Ok(());
        }
        let key = match kind.get("key").and_then(Value::as_str) {
            Some("sync_calls") => SyncCounter::Calls,
            Some("sync_failures") => SyncCounter::Failures,
            Some("sync_elapsed_ns") => SyncCounter::ElapsedNs,
            _ => return Ok(()),
        };
        anyhow::ensure!(self.open.contains(&root), "IO counter outside operation");
        let value = kind
            .get("value")
            .and_then(Value::as_u64)
            .context("IO counter value absent")?;
        anyhow::ensure!(
            self.sync
                .entry(root)
                .or_default()
                .insert(key, value)
                .is_none(),
            "duplicate IO summary counter"
        );
        Ok(())
    }

    fn finish(&self, footer: &Value, publication: bool) -> Result<&BTreeMap<SyncCounter, u64>> {
        anyhow::ensure!(footer.get("status").and_then(Value::as_str) == Some("complete"));
        anyhow::ensure!(footer.get("close_reason").and_then(Value::as_str) == Some("normal"));
        for key in [
            "events_dropped",
            "events_unwritten",
            "open_spans",
            "producer_leases",
        ] {
            anyhow::ensure!(
                footer.get(key).and_then(Value::as_u64) == Some(0),
                "nonzero or absent child footer {key}"
            );
        }
        for key in [
            "events_attempted",
            "events_accepted",
            "events_written",
            "last_output_seq",
        ] {
            anyhow::ensure!(
                footer.get(key).and_then(Value::as_u64) == Some(self.count),
                "child footer count mismatch: {key}"
            );
        }
        anyhow::ensure!(
            self.open.is_empty()
                && self.evidence.contains(&Evidence::Boot)
                && self.evidence.contains(&Evidence::Request),
            "child lifecycle or request trace incomplete"
        );
        anyhow::ensure!(
            !publication
                || (self.evidence.contains(&Evidence::Build)
                    && self.evidence.contains(&Evidence::Publish)),
            "successful Native build/publication spans beneath identified request root absent"
        );
        let mut selected = None;
        for (root, counters) in &self.sync {
            anyhow::ensure!(
                self.boot_roots.contains(root),
                "quiescent IO root lacks runtime boot"
            );
            anyhow::ensure!(
                counters.len() == 3
                    && counters.contains_key(&SyncCounter::Calls)
                    && counters.contains_key(&SyncCounter::Failures)
                    && counters.contains_key(&SyncCounter::ElapsedNs),
                "quiescent IO summary is split or incomplete"
            );
            anyhow::ensure!(
                selected.replace(counters).is_none(),
                "duplicate complete IO root"
            );
        }
        let sync = selected.context("quiescent IO summary absent")?;
        let calls = sync
            .get(&SyncCounter::Calls)
            .context("quiescent IO calls absent")?;
        let failures = sync
            .get(&SyncCounter::Failures)
            .context("quiescent IO failures absent")?;
        let _elapsed = sync
            .get(&SyncCounter::ElapsedNs)
            .context("quiescent IO elapsed absent")?;
        anyhow::ensure!(
            *failures == 0 && (!publication || *calls > 0),
            "child install IO did not complete successfully"
        );
        Ok(sync)
    }
}

pub(super) fn verify(path: &Path, publication: bool, max_bytes: u64) -> Result<Value> {
    let path_before = fs::symlink_metadata(path).context("stat Native child capture path")?;
    anyhow::ensure!(
        path_before.is_file(),
        "Native child capture is not a regular file"
    );
    let epoch = FileEpoch::of(&path_before);
    // A concurrent path replacement with a FIFO must not block validation.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
        .open(path)
        .context("open Native child capture")?;
    let descriptor_before = file
        .metadata()
        .context("stat opened Native child capture")?;
    anyhow::ensure!(
        descriptor_before.is_file() && FileEpoch::of(&descriptor_before) == epoch,
        "Native child capture path changed before validation"
    );
    anyhow::ensure!(
        descriptor_before.permissions().mode() & 0o777 == 0o600,
        "Native child capture permissions must be private"
    );
    anyhow::ensure!(epoch.size <= max_bytes, "Native child capture byte bound");
    let mut reader = BufReader::new(file.take(max_bytes));
    let mut line = Vec::new();
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let read = read_bounded_line(&mut reader, &mut line)?;
    anyhow::ensure!(read > 0, "child capture header absent");
    hash.update(&line);
    bytes = bytes
        .checked_add(u64::try_from(read)?)
        .context("child capture byte count overflow")?;
    let header: Value = serde_json::from_slice(&line)?;
    anyhow::ensure!(header.get("record_type").and_then(Value::as_str) == Some("header"));
    let schema_version = header
        .get("schema_version")
        .and_then(Value::as_u64)
        .context("child capture header schema version absent")?;
    anyhow::ensure!(
        matches!(schema_version, 2..=5),
        "unsupported child capture schema version"
    );
    let id = header
        .get("capture_id")
        .and_then(Value::as_str)
        .context("child capture ID absent")?;
    anyhow::ensure!(id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()));
    for key in ["stream_id", "clock_domain", "process_instance"] {
        anyhow::ensure!(
            header.get(key).and_then(Value::as_str) == Some(id),
            "child clock identity mismatch"
        );
    }
    let mut spans = Spans::default();
    let mut footer = None;
    loop {
        let read = read_bounded_line(&mut reader, &mut line)?;
        if read == 0 {
            break;
        }
        hash.update(&line);
        bytes = bytes
            .checked_add(u64::try_from(read)?)
            .context("child capture byte count overflow")?;
        anyhow::ensure!(footer.is_none(), "invalid child capture record boundary");
        let row: Value = serde_json::from_slice(&line)?;
        let row = quanta_index_searchd::trace_wire::normalize_record(schema_version, row)?;
        anyhow::ensure!(
            row.get("capture_id").and_then(Value::as_str) == Some(id),
            "child capture ID mismatch"
        );
        match row.get("record_type").and_then(Value::as_str) {
            Some("footer") => footer = Some(row),
            Some("event") => spans.event(&row)?,
            _ => anyhow::bail!("unexpected child capture record"),
        }
    }
    let footer = footer.context("child capture footer absent")?;
    anyhow::ensure!(
        footer.get("schema_version").and_then(Value::as_u64) == Some(schema_version),
        "child capture footer schema version mismatch"
    );
    let sync = spans.finish(&footer, publication)?;
    let descriptor_after = reader
        .get_ref()
        .get_ref()
        .metadata()
        .context("restat opened Native child capture")?;
    let path_after = fs::symlink_metadata(path).context("restat Native child capture path")?;
    anyhow::ensure!(
        bytes == epoch.size
            && FileEpoch::of(&descriptor_after) == epoch
            && path_after.is_file()
            && FileEpoch::of(&path_after) == epoch,
        "Native child capture changed during validation"
    );
    Ok(serde_json::json!({
        "capture_id":id,
        "sha256":format!("{:x}", hash.finalize()),
        "bytes":bytes,
        "sync_calls":sync.get(&SyncCounter::Calls).context("IO calls absent")?,
        "sync_failures":sync.get(&SyncCounter::Failures).context("IO failures absent")?,
        "sync_elapsed_ns":sync.get(&SyncCounter::ElapsedNs).context("IO elapsed absent")?,
    }))
}

#[cfg(test)]
pub(super) fn write_balanced_fixture(path: &Path) -> Result<()> {
    tests::fixture(path, tests::Fixture::Balanced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Clone, Copy)]
    pub(super) enum Fixture {
        Balanced,
        MissingEnd,
        WrongCount,
        MissingIo,
        FailedBuild,
        AbortedPublish,
        ForeignBuild,
        ForeignPublish,
    }

    pub(super) fn fixture(path: &Path, case: Fixture) -> Result<()> {
        fixture_with_versions(path, case, 2, 2)
    }

    fn fixture_with_versions(
        path: &Path,
        case: Fixture,
        header_version: u64,
        footer_version: u64,
    ) -> Result<()> {
        let id = "11".repeat(16);
        let mut kinds = vec![
            json!({"type":"span_begin","span_id":1,"stage":"operation"}),
            json!({"type":"span_begin","span_id":2,"parent_span_id":1,"stage":"runtime_boot"}),
            json!({"type":"span_end","span_id":2,"outcome":"ok"}),
            json!({"type":"span_begin","span_id":3,"stage":"request","root_kind":"request"}),
            json!({"type":"request_identity","span_id":3}),
            json!({"type":"span_begin","span_id":4,"parent_span_id":3,"stage":"build_prepare"}),
            json!({"type":"span_end","span_id":4,"outcome":"ok"}),
            json!({"type":"span_begin","span_id":5,"parent_span_id":3,"stage":"native_publish"}),
            json!({"type":"span_end","span_id":5,"outcome":"ok"}),
            json!({"type":"span_end","span_id":3,"outcome":"ok"}),
            json!({"type":"counter","span_id":1,"key":"sync_calls","value":3}),
            json!({"type":"counter","span_id":1,"key":"sync_failures","value":0}),
            json!({"type":"counter","span_id":1,"key":"sync_elapsed_ns","value":27}),
            json!({"type":"span_end","span_id":1,"outcome":"ok"}),
        ];
        if matches!(case, Fixture::MissingEnd) {
            let _removed = kinds.pop();
        }
        if matches!(case, Fixture::MissingIo) {
            kinds.retain(|kind| kind.get("type").and_then(Value::as_str) != Some("counter"));
        }
        let terminal = match case {
            Fixture::FailedBuild => Some((4, "error")),
            Fixture::AbortedPublish => Some((5, "aborted")),
            Fixture::Balanced
            | Fixture::MissingEnd
            | Fixture::WrongCount
            | Fixture::MissingIo
            | Fixture::ForeignBuild
            | Fixture::ForeignPublish => None,
        };
        if let Some((span_id, outcome)) = terminal {
            for (target, terminal) in [(span_id, outcome), (1, "error")] {
                let end = kinds
                    .iter_mut()
                    .find(|kind| {
                        kind.get("type").and_then(Value::as_str) == Some("span_end")
                            && kind.get("span_id").and_then(Value::as_u64) == Some(target)
                    })
                    .context("fixture terminal absent")?;
                let _previous = end
                    .as_object_mut()
                    .context("fixture terminal is not an object")?
                    .insert("outcome".into(), json!(terminal));
            }
        }
        let foreign = match case {
            Fixture::ForeignBuild => Some(4),
            Fixture::ForeignPublish => Some(5),
            Fixture::Balanced
            | Fixture::MissingEnd
            | Fixture::WrongCount
            | Fixture::MissingIo
            | Fixture::FailedBuild
            | Fixture::AbortedPublish => None,
        };
        if let Some(span_id) = foreign {
            let first_witness = kinds
                .iter()
                .position(|kind| kind.get("span_id").and_then(Value::as_u64) == Some(4))
                .context("fixture build witness absent")?;
            kinds.insert(
                first_witness,
                json!({"type":"span_begin","span_id":6,"stage":"request","root_kind":"request"}),
            );
            let witness = kinds
                .iter_mut()
                .find(|kind| {
                    kind.get("type").and_then(Value::as_str) == Some("span_begin")
                        && kind.get("span_id").and_then(Value::as_u64) == Some(span_id)
                })
                .context("fixture target witness absent")?;
            let _previous = witness
                .as_object_mut()
                .context("fixture witness is not an object")?
                .insert("parent_span_id".into(), json!(6));
            let request_end = kinds
                .iter()
                .position(|kind| {
                    kind.get("type").and_then(Value::as_str) == Some("span_end")
                        && kind.get("span_id").and_then(Value::as_u64) == Some(3)
                })
                .context("fixture request end absent")?;
            kinds.insert(
                request_end,
                json!({"type":"span_end","span_id":6,"outcome":"ok"}),
            );
        }
        let mut rows = vec![
            json!({"record_type":"header","schema_version":header_version,"capture_id":id,
            "stream_id":id,"clock_domain":id,"process_instance":id}),
        ];
        let mut count = 0_u64;
        for kind in kinds {
            count = count.checked_add(1).context("fixture count overflow")?;
            rows.push(
                json!({"record_type":"event","capture_id":id,"output_seq":count,"kind":kind}),
            );
        }
        let declared = count
            .checked_add(u64::from(matches!(case, Fixture::WrongCount)))
            .context("fixture footer overflow")?;
        rows.push(json!({"record_type":"footer","schema_version":footer_version,"capture_id":id,"status":"complete","close_reason":"normal",
            "events_dropped":0,"events_unwritten":0,"open_spans":0,"producer_leases":0,
            "events_attempted":declared,"events_accepted":declared,"events_written":declared,"last_output_seq":declared}));
        let text = rows
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join("\n");
        std::fs::write(path, text)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    fn v5_literal_rows() -> Vec<Value> {
        let id = "11111111111111111111111111111111";
        vec![
            json!({"record_type":"header","schema_version":5,"capture_id":id,
                "stream_id":id,"clock_domain":id,"process_instance":id,
                "started_ns":0,"max_record_bytes":4096,
                "max_counters_per_event":1,"coverage":[]}),
            json!(["e", 1, id, 100, ["b", 1, null, "operation", 1, "operation"]]),
            json!(["e", 2, id, 101, ["b", 2, 1, "runtime_boot", 1, null]]),
            json!(["e", 3, id, 102, ["e", 2, "ok", null]]),
            json!(["e", 4, id, 103, ["b", 3, null, "request", 1, "request"]]),
            json!(["e",5,id,104,{"type":"request_identity","span_id":3,
                "request_id":1,"plane":"query","connection_id":1}]),
            json!(["e", 6, id, 105, ["b", 4, 3, "build_prepare", 1, null]]),
            json!(["e", 7, id, 106, ["e", 4, "ok", null]]),
            json!(["e", 8, id, 107, ["b", 5, 3, "native_publish", 1, null]]),
            json!(["e", 9, id, 108, ["e", 5, "ok", null]]),
            json!(["e", 10, id, 109, ["e", 3, "ok", null]]),
            json!(["e", 11, id, 110, ["c", 1, "sync_calls", 3]]),
            json!(["e", 12, id, 111, ["c", 1, "sync_failures", 0]]),
            json!(["e", 13, id, 112, ["c", 1, "sync_elapsed_ns", 27]]),
            json!(["e", 14, id, 113, ["e", 1, "ok", null]]),
            json!({"record_type":"footer","schema_version":5,"capture_id":id,
                "closed_ns":114,"status":"complete","close_reason":"normal","events_dropped":0,
                "events_unwritten":0,"open_spans":0,"producer_leases":0,
                "events_attempted":14,"events_accepted":14,"events_written":14,
                "last_output_seq":14}),
        ]
    }

    fn write_v5_rows(path: &Path, rows: &[Value]) -> Result<()> {
        let mut text = rows
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join("\n");
        text.push('\n');
        std::fs::write(path, text)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    fn v5_rows_with_extra_tail(extras: &[Value], final_seq: u64) -> Result<Vec<Value>> {
        let mut rows = v5_literal_rows();
        let mut footer = rows.pop().context("literal footer absent")?;
        let _original_terminal = rows.pop().context("literal operation terminal absent")?;
        rows.extend_from_slice(extras);
        rows.push(json!([
            "e",
            final_seq,
            "11111111111111111111111111111111",
            160,
            ["e", 1, "ok", null]
        ]));
        let fields = footer
            .as_object_mut()
            .context("literal footer is not an object")?;
        for name in [
            "events_attempted",
            "events_accepted",
            "events_written",
            "last_output_seq",
        ] {
            let _old = fields.insert(name.into(), json!(final_seq));
        }
        let _old = fields.insert("closed_ns".into(), json!(161));
        rows.push(footer);
        Ok(rows)
    }

    fn v5_kinds() -> Result<Vec<Value>> {
        let rows = v5_literal_rows();
        rows.get(1..rows.len().saturating_sub(1))
            .context("literal events absent")?
            .iter()
            .map(|row| row.get(4).cloned().context("literal event kind absent"))
            .collect()
    }

    fn v5_rows_from_kinds(kinds: &[Value]) -> Result<Vec<Value>> {
        let id = "11111111111111111111111111111111";
        let mut original = v5_literal_rows();
        let mut footer = original.pop().context("literal footer absent")?;
        let header = original.remove(0);
        let mut rows = vec![header];
        for (index, kind) in kinds.iter().enumerate() {
            let seq = u64::try_from(index)?
                .checked_add(1)
                .context("event overflow")?;
            let ts = 100_u64.checked_add(seq).context("event time overflow")?;
            rows.push(json!(["e", seq, id, ts, kind]));
        }
        let count = u64::try_from(kinds.len())?;
        let fields = footer.as_object_mut().context("literal footer malformed")?;
        for name in [
            "events_attempted",
            "events_accepted",
            "events_written",
            "last_output_seq",
        ] {
            let _old = fields.insert(name.into(), json!(count));
        }
        let closed_ns = 101_u64
            .checked_add(count)
            .context("capture close time overflow")?;
        let _old = fields.insert("closed_ns".into(), json!(closed_ns));
        rows.push(footer);
        Ok(rows)
    }

    #[test]
    fn v5_independent_settings_roots_preserve_one_lifecycle_io_summary() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("aux-settings-v5.jsonl");
        let auxiliary = [
            json!(["b", 6, null, "operation", 1, "operation"]),
            json!(["b", 7, 6, "sqlite_work", 27, null]),
            json!(["e", 7, "ok", null]),
            json!(["b", 8, 6, "sqlite_work", 28, null]),
            json!(["e", 8, "ok", null]),
            json!(["e", 6, "ok", null]),
        ];
        for before_io_root in [true, false] {
            let mut kinds = v5_kinds()?;
            if before_io_root {
                drop(kinds.splice(0..0, auxiliary.iter().cloned()));
            } else {
                kinds.extend(auxiliary.iter().cloned());
            }
            write_v5_rows(&path, &v5_rows_from_kinds(&kinds)?)?;
            let verified = verify(&path, true, MAX_BYTES)?;
            anyhow::ensure!(verified.get("sync_calls").and_then(Value::as_u64) == Some(3));
            anyhow::ensure!(verified.get("sync_failures").and_then(Value::as_u64) == Some(0));
            anyhow::ensure!(verified.get("sync_elapsed_ns").and_then(Value::as_u64) == Some(27));
        }
        Ok(())
    }

    #[test]
    fn v5_io_summary_cannot_split_duplicate_or_come_from_closed_root() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("io-roots-v5.jsonl");
        let base = v5_kinds()?;
        let auxiliary_open = [
            json!(["b", 6, null, "operation", 1, "operation"]),
            json!(["b", 7, 6, "runtime_boot", 1, null]),
            json!(["e", 7, "ok", null]),
        ];
        let mut split = base.clone();
        drop(split.splice(1..1, auxiliary_open.iter().cloned()));
        let elapsed = split
            .iter_mut()
            .find(|kind| kind.get(2).and_then(Value::as_str) == Some("sync_elapsed_ns"))
            .context("literal elapsed counter absent")?;
        *elapsed = json!(["c", 6, "sync_elapsed_ns", 27]);
        split.push(json!(["e", 6, "ok", null]));
        write_v5_rows(&path, &v5_rows_from_kinds(&split)?)?;
        anyhow::ensure!(
            verify(&path, true, MAX_BYTES).is_err(),
            "split IO roots accepted"
        );

        let mut duplicate = base.clone();
        duplicate.extend(auxiliary_open.iter().cloned());
        duplicate.extend([
            json!(["c", 6, "sync_calls", 1]),
            json!(["c", 6, "sync_failures", 0]),
            json!(["c", 6, "sync_elapsed_ns", 1]),
            json!(["e", 6, "ok", null]),
        ]);
        write_v5_rows(&path, &v5_rows_from_kinds(&duplicate)?)?;
        anyhow::ensure!(
            verify(&path, true, MAX_BYTES).is_err(),
            "duplicate complete IO root accepted"
        );

        let mut closed = base;
        let elapsed = closed.remove(12);
        closed.push(elapsed);
        write_v5_rows(&path, &v5_rows_from_kinds(&closed)?)?;
        anyhow::ensure!(
            verify(&path, true, MAX_BYTES).is_err(),
            "closed operation root accepted IO counter"
        );
        Ok(())
    }

    #[test]
    fn v5_request_or_child_counter_cannot_impersonate_operation_io_root() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("foreign-io-v5.jsonl");
        for (target, insertion) in [(2, 2), (3, 5)] {
            let mut kinds = v5_kinds()?;
            let elapsed = kinds.remove(12);
            let mut elapsed = elapsed;
            *elapsed.get_mut(1).context("literal counter owner absent")? = json!(target);
            kinds.insert(insertion, elapsed);
            write_v5_rows(&path, &v5_rows_from_kinds(&kinds)?)?;
            anyhow::ensure!(
                verify(&path, true, MAX_BYTES).is_err(),
                "non-operation counter replaced quiescent IO summary"
            );
        }
        Ok(())
    }

    #[test]
    fn child_capture_requires_balanced_events_and_independent_footer_counts() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("child.jsonl");
        fixture(&path, Fixture::Balanced)?;
        let verified = verify(&path, true, MAX_BYTES)?;
        anyhow::ensure!(verified.get("sync_calls").and_then(Value::as_u64) == Some(3));
        fixture(&path, Fixture::MissingEnd)?;
        anyhow::ensure!(
            verify(&path, true, MAX_BYTES).is_err(),
            "forged open_spans0 hid an unclosed span"
        );
        fixture(&path, Fixture::MissingIo)?;
        anyhow::ensure!(
            verify(&path, true, MAX_BYTES).is_err(),
            "complete footer hid missing IO metrics"
        );
        fixture(&path, Fixture::WrongCount)?;
        anyhow::ensure!(
            verify(&path, true, MAX_BYTES).is_err(),
            "footer replaced actual event count"
        );
        Ok(())
    }

    #[test]
    fn closed_child_capture_requires_supported_matching_schema_versions() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("child.jsonl");
        for version in [2, 3, 4] {
            fixture_with_versions(&path, Fixture::Balanced, version, version)?;
            let verified = verify(&path, true, MAX_BYTES)?;
            anyhow::ensure!(
                verified.get("sync_calls").and_then(Value::as_u64) == Some(3),
                "supported child version lost the closed fixture: {version}"
            );
        }
        for (header, footer) in [(1, 1), (5, 5), (4, 3), (2, 4)] {
            fixture_with_versions(&path, Fixture::Balanced, header, footer)?;
            anyhow::ensure!(
                verify(&path, true, MAX_BYTES).is_err(),
                "unsupported or mismatched child schema accepted: {header}/{footer}"
            );
        }
        Ok(())
    }

    #[test]
    fn v5_child_capture_preserves_literal_flow_and_rejects_tuple_aliases() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("child-v5.jsonl");
        let rows = v5_literal_rows();
        write_v5_rows(&path, &rows)?;
        let verified = verify(&path, true, MAX_BYTES)?;
        anyhow::ensure!(verified.get("sync_calls").and_then(Value::as_u64) == Some(3));
        anyhow::ensure!(verified.get("sync_elapsed_ns").and_then(Value::as_u64) == Some(27));

        for malformed in [
            json!([
                "e",
                1,
                "11111111111111111111111111111111",
                100,
                ["b", 1, null, "operation", 1]
            ]),
            json!({"record_type":"event","output_seq":1,"capture_id":"11111111111111111111111111111111",
                "ts_ns":100,"kind":{"type":"span_begin","span_id":1,"stage":"operation"}}),
            json!([
                "e",
                1,
                "22222222222222222222222222222222",
                100,
                ["b", 1, null, "operation", 1, "operation"]
            ]),
            json!([
                "e",
                2,
                "11111111111111111111111111111111",
                100,
                ["b", 1, null, "operation", 1, "operation"]
            ]),
        ] {
            let mut changed = rows.clone();
            let first_event = changed.get_mut(1).context("literal event absent")?;
            *first_event = malformed;
            write_v5_rows(&path, &changed)?;
            anyhow::ensure!(
                verify(&path, true, MAX_BYTES).is_err(),
                "malformed V5 tuple or alias bypassed child capture validation"
            );
        }
        Ok(())
    }

    #[test]
    fn v5_child_rejects_unknown_common_vocabulary_with_valid_flow_controls() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("child-v5-vocabulary.jsonl");
        let id = "11111111111111111111111111111111";
        for (extra, final_seq, accepted) in [
            (
                vec![json!(["e", 14, id, 114, ["c", 1, "files_read", 1]])],
                15,
                true,
            ),
            (
                vec![json!([
                    "e",
                    14,
                    id,
                    114,
                    ["c", 1, "unknown_counter_key", 1]
                ])],
                15,
                false,
            ),
            (
                vec![
                    json!(["e", 14, id, 114, ["b", 6, 1, "query_plan", 1, null]]),
                    json!(["e", 15, id, 115, ["e", 6, "ok", null]]),
                ],
                16,
                true,
            ),
            (
                vec![
                    json!(["e", 14, id, 114, ["b", 6, 1, "unknown_stage", 1, null]]),
                    json!(["e", 15, id, 115, ["e", 6, "ok", null]]),
                ],
                16,
                false,
            ),
        ] {
            let rows = v5_rows_with_extra_tail(&extra, final_seq)?;
            write_v5_rows(&path, &rows)?;
            let result = verify(&path, true, MAX_BYTES);
            anyhow::ensure!(
                result.is_ok() == accepted,
                "V5 vocabulary decision differed from fixed independent flow control"
            );
        }
        Ok(())
    }

    #[test]
    fn publication_witness_requires_successful_terminal_beneath_identified_request_root()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("child.jsonl");
        for case in [
            Fixture::FailedBuild,
            Fixture::AbortedPublish,
            Fixture::ForeignBuild,
            Fixture::ForeignPublish,
        ] {
            fixture(&path, case)?;
            let error = verify(&path, true, MAX_BYTES)
                .err()
                .context("malformed publication witness was accepted")?;
            anyhow::ensure!(
                error
                    .to_string()
                    .contains("successful Native build/publication spans"),
                "publication witness failed for unrelated reason: {error:#}"
            );
            // A failed product may still have a complete diagnostic capture;
            // the successful-publication witness is required only by its oracle.
            if matches!(case, Fixture::FailedBuild | Fixture::AbortedPublish) {
                let _failed_capture = verify(&path, false, MAX_BYTES)?;
            }
        }
        Ok(())
    }
}
