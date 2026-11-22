#[expect(
    clippy::print_stdout,
    reason = "Cargo reads build-script cfg and rerun directives from stdout"
)]
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=QUANTA_INDEX_TEST_REQUIRE_PINNED_POTION_MODEL");
    println!("cargo::rustc-check-cfg=cfg(quanta_index_test_pinned_potion_model)");

    if std::env::var("QUANTA_INDEX_TEST_REQUIRE_PINNED_POTION_MODEL")
        .is_ok_and(|value| value == "1")
    {
        println!("cargo::rustc-cfg=quanta_index_test_pinned_potion_model");
    }
}
