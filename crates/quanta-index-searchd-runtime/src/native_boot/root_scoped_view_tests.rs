//! Real Q UDS observations only. No Source/Host/installation authority is issued.
use super::*;

#[test]
fn actual_root_scoped_view_live_writer_matches_offline_nonce_and_pinned_inode() -> TestResult {
    let mut fixture = prepare_native_fixture()?;
    let named = fs::metadata(&fixture.root)?;
    let observed = fixture
        .client
        .observe_root_view(ViewSelection::Exact(fixture.native_view), &request())?;
    let ordinary = fixture
        .client
        .read_view(ViewSelection::Exact(fixture.native_view), &request())?;
    require(
        observed.view == ordinary,
        "root-scoped view differs from canonical ReadView",
    )?;
    require(
        observed.root_nonce != Digest([0; 32])
            && observed.root_device == named.dev()
            && observed.root_inode == named.ino(),
        "root-scoped observation differs from actual held inode",
    )?;
    let (later, _record) = super::capture_tests::advance_source(&fixture)?;
    let current = fixture.client.observe_root_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &request(),
    )?;
    require(
        current.view.identity == later && current.view.source != observed.view.source,
        "root-scoped read did not follow actual SourceAdvance",
    )?;
    require(
        current.root_nonce == observed.root_nonce
            && current.root_device == observed.root_device
            && current.root_inode == observed.root_inode,
        "same live root changed its incarnation",
    )?;
    fixture.running.stop()?;
    boot::with_admitted_control_budget(&fixture.config, request(), false, |budget, _cache| {
        boot::with_boot_authority(&fixture.config, budget, |authority, budget| {
            let snapshot = quanta_index_catalog::verify_native_boot_catalog(
                &fixture.root,
                authority.io.as_ref(),
                budget,
            )?;
            require(
                snapshot.selected.is_none(),
                "pre-BOOT fixture unexpectedly selected BOOT",
            )
            .map_err(|error| anyhow!(error.to_string()))?;
            require(
                Digest(snapshot.root_nonce) == observed.root_nonce,
                "live observation differs from canonical offline stamp",
            )
            .map_err(|error| anyhow!(error.to_string()))?;
            boot::require_retired(&authority)?;
            Ok(())
        })
    })?;
    Ok(())
}

#[test]
fn actual_root_scoped_view_rejects_replaced_named_root() -> TestResult {
    let fixture = prepare_native_fixture()?;
    let moved = fixture.root.with_extension("held-original");
    fs::rename(&fixture.root, &moved)?;
    fs::create_dir(&fixture.root)?;
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700))?;
    // Fault setup only: the live service still owns the original root inode.
    let observed = fixture
        .client
        .observe_root_view(ViewSelection::Exact(fixture.native_view), &request());
    let remove = fs::remove_dir(&fixture.root);
    let restore = fs::rename(&moved, &fixture.root);
    remove?;
    restore?;
    require(
        observed.is_err(),
        "replacement root pathname authenticated the original daemon",
    )?;
    fixture.running.finish()?;
    Ok(())
}
