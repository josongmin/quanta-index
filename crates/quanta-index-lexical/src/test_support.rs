//! Test fixture with the same track/family/generation topology as production.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct GenerationFixture {
    track: tempfile::TempDir,
    generation: PathBuf,
}

impl GenerationFixture {
    pub(crate) fn track_path(&self) -> &Path {
        self.track.path()
    }

    pub(crate) fn path(&self) -> &Path {
        &self.generation
    }
}

pub(crate) fn generation_fixture() -> io::Result<GenerationFixture> {
    let track = tempfile::tempdir()?;
    let generation = track.path().join("family").join("g1");
    std::fs::create_dir_all(&generation)?;
    Ok(GenerationFixture { track, generation })
}
