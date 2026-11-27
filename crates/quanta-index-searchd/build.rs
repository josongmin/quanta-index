use std::io::Write;

fn main() -> std::io::Result<()> {
    let mut output = std::io::stdout().lock();
    writeln!(output, "cargo::rerun-if-changed=build.rs")?;
    writeln!(
        output,
        "cargo::rerun-if-env-changed=QUANTA_INDEX_TEST_REQUIRE_PINNED_POTION_MODEL"
    )?;
    writeln!(
        output,
        "cargo::rustc-check-cfg=cfg(quanta_index_test_pinned_potion_model)"
    )?;

    if std::env::var("QUANTA_INDEX_TEST_REQUIRE_PINNED_POTION_MODEL")
        .is_ok_and(|value| value == "1")
    {
        writeln!(
            output,
            "cargo::rustc-cfg=quanta_index_test_pinned_potion_model"
        )?;
    }
    Ok(())
}
