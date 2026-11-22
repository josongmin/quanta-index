use std::{error::Error, path::PathBuf};

pub(super) struct Observation {
    directory: PathBuf,
    nonce: String,
    sha256: String,
}

pub(super) fn observe(pid: u32) -> Result<Option<Observation>, Box<dyn Error>> {
    let Some(directory) = std::env::var_os("QUANTA_PROOF_DAEMON_WITNESS_DIR") else {
        if std::env::var_os("QUANTA_PROOF_DAEMON_WITNESS_NONCE").is_some() {
            return Err("daemon witness directory is missing".into());
        }
        return Ok(None);
    };
    let nonce = std::env::var("QUANTA_PROOF_DAEMON_WITNESS_NONCE")?;
    #[cfg(target_os = "linux")]
    {
        use sha2::{Digest as _, Sha256};

        let mut executable = std::fs::File::open(format!("/proc/{pid}/exe"))?;
        let mut hasher = Sha256::new();
        let _hashed_bytes = std::io::copy(&mut executable, &mut hasher)?;
        Ok(Some(Observation {
            directory: directory.into(),
            nonce,
            sha256: format!("{:x}", hasher.finalize()),
        }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _inputs = (pid, directory, nonce);
        Err("daemon executable observation requires Linux".into())
    }
}

pub(super) fn write<const N: usize>(
    observations: [Option<Observation>; N],
    filename: &str,
    test: &str,
    provider_approval_sha256: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let observations: Vec<_> = observations.into_iter().flatten().collect();
    if let Some(observed) = observations.first() {
        if observations.len() != N
            || observations.iter().any(|launch| {
                launch.directory != observed.directory
                    || launch.nonce != observed.nonce
                    || launch.sha256 != observed.sha256
            })
        {
            return Err(
                "daemon witness launches differ in destination, scope or executable".into(),
            );
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(observed.directory.join(filename))?;
        let mut witness = serde_json::json!({
            "schema_version": 1,
            "test": test,
            "nonce": observed.nonce,
            "executables": observations.iter().map(|launch| &launch.sha256).collect::<Vec<_>>(),
        });
        if let Some(approval) = provider_approval_sha256 {
            witness["provider_approval_sha256"] = serde_json::Value::String(approval.into());
        }
        serde_json::to_writer(&file, &witness)?;
        file.sync_all()?;
    }
    Ok(())
}
