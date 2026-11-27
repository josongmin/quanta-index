fn main() {
    println!("cargo:rerun-if-changed=src/linux_quota.c");
    println!("cargo:rerun-if-changed=src/linux_write_domain.c");
    println!("cargo:rerun-if-changed=src/linux_fd_preflight.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        cc::Build::new()
            .file("src/linux_quota.c")
            .file("src/linux_write_domain.c")
            .file("src/linux_fd_preflight.c")
            .warnings(true)
            .compile("quanta_linux_quota");
    }
}
