//! Shared by the analysis tests: scratch folders and the generated drum loop.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A folder for one test's files, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bl-analysis-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn drums_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/drums")
}

/// The generated drum loop (`tests/fixtures/gen/gen_drums.py`), as WAV bytes.
pub fn drum_loop() -> Vec<u8> {
    std::fs::read(drums_dir().join("drum-loop.wav")).unwrap()
}

/// When each hit of the drum loop starts, in seconds.
pub fn drum_hits() -> Vec<f64> {
    std::fs::read_to_string(drums_dir().join("drum-loop.hits"))
        .unwrap()
        .lines()
        .map(|line| line.parse().unwrap())
        .collect()
}
