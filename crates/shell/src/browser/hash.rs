//! Hashing files in the background, so a tagged file's content hash is
//! known without holding up the browser (spec: browser-tags R5).
//!
//! A file that carries a tag keeps its size and its BLAKE3 hash, so when
//! the file is moved or renamed outside Beatlayer its tags pass to the
//! first file of the same size and content that indexing finds.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use bl_media::ContentHash;

/// A file that has been hashed.
#[derive(Clone, Debug)]
pub struct Hashed {
    /// The file.
    pub path: PathBuf,
    /// Its hash as hex, or why it couldn't be read.
    pub hash: Result<String, String>,
}

/// Hashes files on a thread of its own.
#[derive(Debug, Default)]
pub struct Hasher {
    requests: Option<Sender<PathBuf>>,
    results: Option<Receiver<Hashed>>,
}

impl Hasher {
    /// A hasher with no thread yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks for `path` to be hashed. The answer comes in [`Hasher::poll`].
    pub fn request(&mut self, path: &Path) {
        if self.requests.is_none() {
            let (sender, receiver) = mpsc::channel::<PathBuf>();
            let (results, results_receiver) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("bl-browser-hash".to_owned())
                .spawn(move || {
                    while let Ok(path) = receiver.recv() {
                        let hash = ContentHash::of_file(&path)
                            .map(|hash| hash.to_string())
                            .map_err(|error| error.to_string());
                        if results.send(Hashed { path, hash }).is_err() {
                            return;
                        }
                    }
                });
            if spawned.is_err() {
                return;
            }
            self.requests = Some(sender);
            self.results = Some(results_receiver);
        }
        if let Some(sender) = &self.requests {
            let _ = sender.send(path.to_path_buf());
        }
    }

    /// The files hashed since the last call.
    pub fn poll(&mut self) -> Vec<Hashed> {
        self.results
            .as_ref()
            .map(|results| results.try_iter().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_hashed_in_the_background() {
        let dir = std::env::temp_dir().join(format!("bl-browser-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the folder");
        let path = dir.join("a.txt");
        std::fs::write(&path, b"hello").expect("the file");
        let mut hasher = Hasher::new();
        hasher.request(&path);
        let mut answer = None;
        for _ in 0..2000 {
            if let Some(hashed) = hasher.poll().into_iter().next() {
                answer = Some(hashed);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let hashed = answer.expect("the hash");
        assert_eq!(hashed.path, path);
        assert!(hashed.hash.is_ok(), "{:?}", hashed.hash);
    }
}
