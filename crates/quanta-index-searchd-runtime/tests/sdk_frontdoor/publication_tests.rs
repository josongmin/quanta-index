use super::*;

#[test]
fn sdk_publish_frontdoor_routes_ingest_batches() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let lexical_receipt = client.search_corpus().publish(&lexical_batch()?)?;
    let history_receipt = client.history().publish(&history_batch())?;
    let dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let structural_receipt = client.structural().publish(&structural_batch()?)?;
    if lexical_receipt.generation != generation()
        || lexical_receipt.accepted_replace_scopes != 3
        || lexical_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected search-corpus receipt: {lexical_receipt:?}").into());
    }
    if history_receipt.generation != generation()
        || history_receipt.accepted_replace_scopes != 4
        || history_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected history receipt: {history_receipt:?}").into());
    }
    if dirty_receipt.generation != generation()
        || dirty_receipt.accepted_replace_scopes != 1
        || dirty_receipt.accepted_tombstone_scopes != 1
    {
        return Err(format!("unexpected dirty receipt: {dirty_receipt:?}").into());
    }
    if structural_receipt.generation != generation()
        || structural_receipt.accepted_replace_scopes != 1
        || structural_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected structural receipt: {structural_receipt:?}").into());
    }
    fixture.stop()
}
