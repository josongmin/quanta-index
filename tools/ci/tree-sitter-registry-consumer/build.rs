fn main() {
    println!("cargo::rustc-check-cfg=cfg(tree_sitter_registry_consumer)");
    println!("cargo::rustc-cfg=tree_sitter_registry_consumer");
}
