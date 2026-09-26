//! Optional raw native output, never an authority for source or terminal custody.

use std::{fs::OpenOptions, io::Write, path::Path};

const MAX_CAPTURE_BYTES: usize = 256 * 1024;

pub(super) fn write_new_external(path: &Path, payload: &serde_json::Value) -> Result<(), String> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err("parity capture requires an absolute file path".into());
    }
    let parent = path
        .parent()
        .ok_or("capture has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if parent.starts_with(repository) {
        return Err("parity capture must be outside the source repository".into());
    }
    let mut bytes = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > MAX_CAPTURE_BYTES {
        return Err("parity capture exceeds the fixed 256 KiB output bound".into());
    }
    // create_new also refuses an existing symlink. Failure is surfaced to the
    // test, never replaced by a successful missing or partial capture.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

pub(super) fn norms(vectors: &[Vec<f32>]) -> Vec<f64> {
    vectors
        .iter()
        .map(|vector| {
            vector
                .iter()
                .map(|value| f64::from(*value).powi(2))
                .sum::<f64>()
                .sqrt()
        })
        .collect()
}

pub(super) fn cosine_triangle(vectors: &[Vec<f32>]) -> Vec<Vec<f64>> {
    let lengths = norms(vectors);
    vectors
        .iter()
        .zip(&lengths)
        .enumerate()
        .map(|(index, (left, left_norm))| {
            vectors
                .iter()
                .zip(&lengths)
                .skip(index + 1)
                .map(|(right, right_norm)| {
                    left.iter()
                        .zip(right)
                        .map(|(a, b)| (f64::from(*a) / left_norm) * (f64::from(*b) / right_norm))
                        .sum()
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parity_capture_refuses_relative_repository_and_existing_targets() {
        let payload = serde_json::json!({"capture": "test-only"});
        assert!(write_new_external(Path::new("relative.json"), &payload).is_err());
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository");
        assert!(write_new_external(&repo.join("never-created-parity.json"), &payload).is_err());
        let temporary = tempfile::tempdir().expect("temporary");
        let path = temporary.path().join("capture.json");
        write_new_external(&path, &payload).expect("new external capture");
        let bytes = std::fs::read(&path).expect("capture reads");
        assert!(write_new_external(&path, &serde_json::json!({"forged": true})).is_err());
        assert_eq!(std::fs::read(&path).expect("unchanged capture"), bytes);
        #[cfg(unix)]
        {
            let symlink = temporary.path().join("symlink.json");
            std::os::unix::fs::symlink(&path, &symlink).expect("symlink");
            assert!(write_new_external(&symlink, &payload).is_err());
            let alias = temporary.path().join("repository-alias");
            std::os::unix::fs::symlink(&repo, &alias).expect("repository symlink");
            assert!(
                write_new_external(&alias.join("never-created-parity.json"), &payload).is_err()
            );
        }
    }

    #[test]
    fn parity_capture_enforces_bound_and_independent_norm_cosine_oracle() {
        let temporary = tempfile::tempdir().expect("temporary");
        let path = temporary.path().join("too-large.json");
        assert!(
            write_new_external(&path, &serde_json::json!("a".repeat(MAX_CAPTURE_BYTES))).is_err()
        );
        assert!(!path.exists());
        let vectors = vec![vec![3.0, 0.0], vec![0.0, 4.0], vec![3.0, 0.0]];
        assert_eq!(norms(&vectors), vec![3.0, 4.0, 3.0]);
        assert_eq!(
            cosine_triangle(&vectors),
            vec![vec![0.0, 1.0], vec![0.0], vec![]]
        );
    }
}
