#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    let syntax = || {
        Error::new(
            ErrorKind::InvalidInput,
            format!(
                "usage: {} --client-uid <nonzero decimal UID>",
                program.to_string_lossy()
            ),
        )
    };
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--client-uid")) {
        return Err(syntax());
    }
    let uid = args
        .next()
        .ok_or_else(syntax)?
        .to_str()
        .ok_or_else(syntax)?
        .parse::<u32>()
        .map_err(|_| syntax())?;
    if uid == 0 || args.next().is_some() {
        return Err(syntax());
    }
    let listener = quanta_state_root_quota::take_systemd_listener()?;
    quanta_state_root_quota::serve_xfs_project_quota(listener, uid)
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("quanta-state-root-quota-verifier requires Linux");
    std::process::exit(1);
}
