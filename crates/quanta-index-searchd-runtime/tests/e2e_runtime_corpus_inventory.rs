#[path = "common/runtime_corpus_inventory.rs"]
mod runtime_corpus_inventory;

#[test]
fn runtime_corpus_row_inventory_is_pinned() -> anyhow::Result<()> {
    let _corpus = runtime_corpus_inventory::load_pinned_runtime_rows()?;
    Ok(())
}
