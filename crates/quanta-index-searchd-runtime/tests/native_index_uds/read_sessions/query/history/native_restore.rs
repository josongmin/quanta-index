//! An actual old Native History object closure survives child-first transfer
//! into a fresh root, restore FULL, append and destination daemon restart.
use super::*;
use quanta_index_contract::{
    index::NativeHistoryRestoreInputV1,
    native_protocol::{NativeImmutableObjectChildrenV1, NativeImmutableObjectPayloadV1},
};

fn destination_operation(sequence: u64) -> OperationKey {
    OperationKey {
        stream: StreamId([44; 16]),
        ..operation(sequence)
    }
}

pub(super) fn stage_old_history_closure<P: AsRef<std::path::Path>>(
    old: &FileObjectStore,
    client: &NativeUdsClient<P>,
    ingress: IngressRef,
    root: ObjectRef,
) -> TestResult<usize> {
    // A fixture-local postorder walk: every child is staged before its parent.
    let mut pending = vec![(root, false)];
    let mut seen = std::collections::BTreeSet::new();
    let mut staged = 0_usize;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    while let Some((reference, ready)) = pending.pop() {
        if !ready {
            if !seen.insert(reference) {
                continue;
            }
            if seen.len() > 16_384 {
                return Err("History fixture object closure exceeds bound".into());
            }
            let object = old
                .get(reference, &mut work)
                .map_err(|cause| format!("read old History closure {reference:?}: {cause}"))?;
            pending.push((reference, true));
            for child in object.registration.children.iter().rev() {
                pending.push((*child, false));
            }
            continue;
        }
        let object = old
            .get(reference, &mut work)
            .map_err(|cause| format!("read old History closure {reference:?}: {cause}"))?;
        let NativeIndexResponse::Staged(actual) = client.request(
            NativeIndexRequest::StageImmutableObject {
                ingress,
                bytes: NativeImmutableObjectPayloadV1::new(object.payload.clone())?,
                children: NativeImmutableObjectChildrenV1::new(
                    object.registration.children.clone(),
                )?,
            },
            &budget(),
        )?
        else {
            return Err("restored immutable object was not staged".into());
        };
        assert_eq!(actual, reference);
        staged = staged
            .checked_add(1)
            .ok_or("History closure count overflow")?;
    }
    Ok(staged)
}

#[test]
fn native_history_restore_full_append_and_reopen_cross_root() -> TestResult {
    let (full, _, full_input, _) = publish_in_legacy_child()?;
    let expected = cut(&full, None, full_input.fence.final_replay)?;
    let source_root = private_root()?;
    let source_socket = source_root.path().join("native.sock");
    let source_running =
        NativeServiceGuard::start(history_service(source_root.path(), &source_socket)?);
    let old_result = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(&source_socket);
        let configured = create_text_workspace(&client)?;
        let (source, _) = publish_text_owner(
            &client,
            configured,
            2,
            1,
            "native-restore.rs",
            "needle current file",
        )?;
        let old_view = published(&client.submit(
            stage_history(&client, source, 3, &full_input, None, expected.clone())?,
            &budget(),
        )?)?;
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &source_socket,
            ViewSelection::Exact(old_view),
        ))?;
        let mut old_session = sdk.reader().native()?.open([91; 32], &budget())?;
        assert_eq!(old_session.load_profile(&budget())?.view.identity, old_view);
        let old_member = old_session.history_cut(&history_spec(), &budget())?;
        assert_eq!(old_member.cut, expected);
        assert!(!old_session.release(&budget())?);
        let NativeIndexResponse::RootScopedView(old_root) = client.request(
            NativeIndexRequest::ObserveRootView(ViewSelection::Exact(old_view)),
            &budget(),
        )?
        else {
            return Err("old Native root observation absent".into());
        };
        assert_eq!(old_root.view.identity, old_view);
        Ok((old_view, old_member, old_root))
    })();
    let source_stopped = source_running.stop();
    let (old_view, old_member, old_root) = old_result?;
    source_stopped?;

    let source_objects = FileObjectStore::open(source_root.path().join("native-objects"))?;
    let root = private_root()?;
    assert_ne!(source_root.path(), root.path());
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let restored_spec = history_spec();
    let outcome = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(&socket);
        let configured = create_text_workspace(&client)?;
        let (destination_source, _) = publish_text_owner(
            &client,
            configured,
            2,
            1,
            "native-restore.rs",
            "needle new destination file",
        )?;
        let NativeIndexResponse::RootScopedView(destination_root) = client.request(
            NativeIndexRequest::ObserveRootView(ViewSelection::Exact(destination_source)),
            &budget(),
        )?
        else {
            return Err("destination Native root observation absent".into());
        };
        assert_ne!(destination_root.root_nonce, old_root.root_nonce);
        assert_ne!(destination_root.view.source, old_root.view.source);
        let destination = destination_source;
        let destination_source_cut = destination_root.view.source;
        let restore_key = destination_operation(1);
        let ingress = client.begin_input(restore_key, &budget())?;
        assert!(
            stage_old_history_closure(&source_objects, &client, ingress, old_member.member,)? > 1
        );
        let profile = client.stage_profile(
            ingress,
            IndexProfile {
                instances: vec![native_text_spec(), restored_spec.clone()],
            },
            &budget(),
        )?;
        let restore = NativeHistoryRestoreInputV1 {
            wire_version: 3,
            source_root_nonce: old_root.root_nonce,
            source_view: old_view,
            instance: old_member.instance,
            member: old_member.member,
            cut: old_member.cut,
        };
        let input = client.stage_projection_output_with_children(
            ingress,
            &serde_json::to_vec(&restore)?,
            &[restore.member],
            &budget(),
        )?;
        let restored = published(&client.submit(
            UpdateRequest {
                operation: restore_key,
                expected_view: destination,
                kind: UpdateKind::HistoryAdvance(HistoryTransition {
                    instance: restored_spec.instance,
                    expected: None,
                    next: expected.clone(),
                    input,
                    profile,
                }),
            },
            &budget(),
        )?)?;
        let restored_view = client.read_view(ViewSelection::Exact(restored), &budget())?;
        assert_eq!(restored_view.source, destination_source_cut);
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(restored),
        ))?;
        let mut session = sdk.reader().native()?.open([92; 32], &budget())?;
        assert_eq!(session.load_profile(&budget())?.view.identity, restored);
        let restored_member = session.history_cut(&restored_spec, &budget())?;
        assert_eq!(restored_member.cut, expected);
        assert_ne!(restored_member.member, old_member.member);
        let page = session.history(
            &restored_spec,
            query("type:commit needle", HistoryOrderV1::Recency, 4),
            None,
            &budget(),
        )?;
        assert_ids(&page, &[2, 3, 1]);
        assert!(!session.release(&budget())?);

        let mut original = HistoryIngestBatch {
            repo_id: full.repo_id.clone(),
            revision_id: full.revision_id.clone(),
            generation: full.generation,
            manifest_digest: full.manifest_digest.clone(),
            batch_digest: String::new(),
            commits: vec![commit(8, 50_000, "needle restored append", "alice")],
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        };
        stamp_batch_digest_v1(&mut original)?;
        let append_operation = destination_operation(2);
        let append_ingress = client.begin_input(append_operation, &budget())?;
        let append_input = client.stage_projection_output_with_children(
            append_ingress,
            &serde_json::to_vec(&original)?,
            &[],
            &budget(),
        )?;
        let appended = published(&client.submit(
            UpdateRequest {
                operation: append_operation,
                expected_view: restored,
                kind: UpdateKind::HistoryAppend(HistoryAppendV1 {
                    instance: restored_spec.instance,
                    expected: expected.clone(),
                    input: append_input,
                    profile: restored_view.profile,
                }),
            },
            &budget(),
        )?)?;
        Ok((appended, restored_spec, restored_view.source))
    })();
    let stopped = running.stop();
    let (appended, restored_spec, retained_source) = outcome?;
    stopped?;
    drop(source_objects);
    source_root.close()?;

    let reopened = NativeServiceGuard::start(history_service(root.path(), &socket)?);
    let result = (|| -> TestResult {
        let client = NativeUdsClient::new(&socket);
        assert_eq!(
            client
                .read_view(ViewSelection::Exact(appended), &budget())?
                .source,
            retained_source
        );
        let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &socket,
            ViewSelection::Exact(appended),
        ))?;
        let mut session = sdk.reader().native()?.open([93; 32], &budget())?;
        assert_eq!(session.load_profile(&budget())?.view.identity, appended);
        let cut = session.history_cut(&restored_spec, &budget())?;
        assert_eq!(cut.view, appended);
        assert_eq!(cut.cut.durable_epoch, AuxEpochV1::new(2));
        let page = session.history(
            &restored_spec,
            query("type:commit needle", HistoryOrderV1::Recency, 4),
            None,
            &budget(),
        )?;
        assert_ids(&page, &[8, 2, 3, 1]);
        assert!(!session.release(&budget())?);
        Ok(())
    })();
    let stopped = reopened.stop();
    result?;
    stopped?;
    Ok(())
}
