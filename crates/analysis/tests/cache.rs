//! The analysis cache: background analysis kept by content hash, and what
//! keeps a stale or damaged file from being shown (the warping spec, R20
//! and R21).

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bl_analysis::cache::{Analysis, AnalysisCache, AnalysisOptions, AnalysisStatus};
use bl_media::{ContentHash, MediaError, MediaStatus, MediaStore};
use common::{TempDir, drum_hits, drum_loop};

/// Imports bytes written to `name` in `dir`.
fn import(store: &MediaStore, dir: &Path, name: &str, bytes: &[u8]) -> ContentHash {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    store.import(path).unwrap()
}

fn cache(store: &Arc<MediaStore>, options: AnalysisOptions) -> AnalysisCache {
    AnalysisCache::new(Arc::clone(store), options).unwrap()
}

/// The drum loop with its first 0.3 s silenced: other media, other hits.
fn quieter_loop() -> Vec<u8> {
    let mut bytes = drum_loop();
    bytes[44..44 + 28_800].fill(0);
    bytes
}

// Spec: warping R20, R21
#[test]
fn analysis_runs_in_the_background_and_is_kept_by_content_hash() {
    let media = TempDir::new("media");
    let folder = TempDir::new("kept");
    let store = Arc::new(MediaStore::new());
    let hash = import(&store, media.path(), "loop.wav", &drum_loop());

    let first = cache(&store, AnalysisOptions::in_dir(folder.path()));
    // A request never waits: it starts the analysis and says it is pending.
    assert!(matches!(first.request(hash), AnalysisStatus::Pending));
    let analysis = first.wait(hash).unwrap();
    assert_eq!(analysis.transients.len(), drum_hits().len());
    assert!(first.get(hash).is_some());
    assert!(first.file_path(hash).unwrap().is_file());

    // A second session, from another path, loads the file by content hash.
    // Its content is changed to numbers the audio can't give, so only a load
    // can return them.
    let marked = Analysis {
        transients: vec![0.125],
    };
    fs::write(first.file_path(hash).unwrap(), marked.to_file_bytes(hash)).unwrap();
    let elsewhere = TempDir::new("elsewhere");
    let second_store = Arc::new(MediaStore::new());
    let same = import(
        &second_store,
        elsewhere.path(),
        "Copy of loop.wav",
        &drum_loop(),
    );
    assert_eq!(same, hash);
    let second = cache(&second_store, AnalysisOptions::in_dir(folder.path()));
    assert_eq!(*second.wait(hash).unwrap(), marked);
}

// Spec: warping R21
#[test]
fn a_changed_file_never_shows_its_old_analysis() {
    let media = TempDir::new("media");
    let folder = TempDir::new("changed");
    let store = Arc::new(MediaStore::new());
    let hash = import(&store, media.path(), "loop.wav", &drum_loop());
    let cache = cache(&store, AnalysisOptions::in_dir(folder.path()));
    let old = cache.wait(hash).unwrap();
    let file = cache.file_path(hash).unwrap();
    assert!(file.is_file());

    // An editor saves other audio over the file.
    let path = media.path().join("loop.wav");
    fs::write(&path, quieter_loop()).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    assert!(matches!(
        cache.check(hash).unwrap(),
        MediaStatus::Changed { .. }
    ));
    assert!(cache.get(hash).is_none());
    assert!(!file.exists());
    assert!(matches!(
        cache.request(hash),
        AnalysisStatus::Pending | AnalysisStatus::Failed(_)
    ));
    assert!(matches!(
        &*cache.wait(hash).unwrap_err(),
        MediaError::Changed { .. }
    ));

    // The file imported again is new media, analysed on its own.
    let new = store.import(&path).unwrap();
    assert_ne!(new, hash);
    let fresh = cache.wait(new).unwrap();
    assert_ne!(fresh.transients, old.transients);
    assert!(fresh.transients[0] > 0.3, "{:?}", fresh.transients);
}

// Spec: warping R21
#[test]
fn a_damaged_or_foreign_analysis_file_is_rebuilt() {
    let media = TempDir::new("media");
    let folder = TempDir::new("damaged");
    let store = Arc::new(MediaStore::new());
    let hash = import(&store, media.path(), "loop.wav", &drum_loop());
    let good = {
        let cache = cache(&store, AnalysisOptions::in_dir(folder.path()));
        let analysis = cache.wait(hash).unwrap();
        let bytes = fs::read(cache.file_path(hash).unwrap()).unwrap();
        (analysis, bytes)
    };
    let file = AnalysisCache::new(Arc::clone(&store), AnalysisOptions::in_dir(folder.path()))
        .unwrap()
        .file_path(hash)
        .unwrap();

    let mut cut = good.1.clone();
    cut.truncate(cut.len() - 3);
    let mut flipped = good.1.clone();
    *flipped.last_mut().unwrap() ^= 1;
    let other = Analysis {
        transients: vec![0.5],
    }
    .to_file_bytes(ContentHash::of_bytes(b"other media"));
    for damaged in [
        b"not an analysis file".to_vec(),
        cut,
        flipped,
        other,
        Vec::new(),
    ] {
        fs::write(&file, &damaged).unwrap();
        let session = cache(&store, AnalysisOptions::in_dir(folder.path()));
        assert_eq!(*session.wait(hash).unwrap(), *good.0);
        assert_eq!(fs::read(&file).unwrap(), good.1, "replaced by a good file");
    }
}

// Spec: warping R21
#[test]
fn a_memory_only_cache_writes_no_files() {
    let media = TempDir::new("media");
    let store = Arc::new(MediaStore::new());
    let hash = import(&store, media.path(), "loop.wav", &drum_loop());
    let cache = cache(&store, AnalysisOptions::memory_only());
    assert!(cache.dir().is_none());
    assert!(cache.file_path(hash).is_none());
    assert_eq!(
        cache.wait(hash).unwrap().transients.len(),
        drum_hits().len()
    );
    // The loop is the only file anywhere in the test's folder.
    assert_eq!(fs::read_dir(media.path()).unwrap().count(), 1);
}
