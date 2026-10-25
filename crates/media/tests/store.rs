//! The media store: import, content checks, shared loading, sample-rate
//! conversion on load, and relinking moved files by content.

use crate::common;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bl_media::{
    ContentHash, Decoder, Edit, EditKind, MediaError, MediaStatus, MediaStore, converted_len,
    decode_file,
};
use common::{TempDir, fixtures_dir};

/// Copies a fixture into a folder, so the test can move and change it.
fn copy_fixture(name: &str, to: &Path) -> PathBuf {
    fs::create_dir_all(to).unwrap();
    let path = to.join(name);
    fs::copy(fixtures_dir().join(name), &path).unwrap();
    path
}

#[test]
fn import_hashes_the_bytes_and_reads_the_header() {
    // Spec: audio-import R9
    let dir = TempDir::new("store-import");
    let path = copy_fixture("sine-44k1-s16-2ch.flac", dir.path());
    let store = MediaStore::new();
    let hash = store.import(&path).unwrap();
    assert_eq!(hash, ContentHash::of_file(&path).unwrap());

    let entry = store.entry(hash).unwrap();
    assert_eq!(entry.path, path);
    assert_eq!(entry.size, fs::metadata(&path).unwrap().len());
    let info = entry.info.unwrap();
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert_eq!(store.info(hash).unwrap(), info);

    // The same content imported from elsewhere is the same media.
    let copy = copy_fixture("sine-44k1-s16-2ch.flac", &dir.path().join("copy"));
    assert_eq!(store.import(&copy).unwrap(), hash);
    assert_eq!(store.entries().len(), 1);
    assert_eq!(store.entry(hash).unwrap().path, path);

    // Different content is different media.
    let other = copy_fixture("sine-44k1-s16-2ch.wav", dir.path());
    assert_ne!(store.import(&other).unwrap(), hash);
    assert_eq!(store.entries().len(), 2);
}

#[test]
fn importing_a_file_that_isnt_audio_fails() {
    let dir = TempDir::new("store-not-audio");
    let path = dir.path().join("notes.wav");
    fs::write(&path, "not audio").unwrap();
    let store = MediaStore::new();
    assert!(store.import(&path).is_err());
    assert!(store.entries().is_empty());
}

#[test]
fn loads_share_decoded_audio_while_it_is_in_use() {
    let dir = TempDir::new("store-load");
    let path = copy_fixture("sweep-48k-s24-1ch.flac", dir.path());
    let store = MediaStore::new();
    let hash = store.import(&path).unwrap();

    let first = store.load(hash).unwrap();
    assert_eq!(*first, decode_file(&path).unwrap());
    let second = store.load(hash).unwrap();
    assert!(Arc::ptr_eq(&first, &second), "a second load decoded again");

    // Once nothing holds it, it's freed, and a load decodes it afresh.
    let weak = Arc::downgrade(&first);
    drop((first, second));
    assert!(weak.upgrade().is_none());
    assert_eq!(*store.load(hash).unwrap(), decode_file(&path).unwrap());
}

#[test]
fn loads_at_the_engine_rate() {
    let dir = TempDir::new("store-rate");
    let path = copy_fixture("sine-44k1-s16-2ch.wav", dir.path());
    let store = MediaStore::new();
    let hash = store.import(&path).unwrap();

    let converted = store.load_at_rate(hash, 48_000).unwrap();
    assert_eq!(converted.sample_rate(), 48_000);
    assert_eq!(converted.channel_count(), 2);
    assert_eq!(converted.frames(), converted_len(11_025, 44_100, 48_000));
    let again = store.load_at_rate(hash, 48_000).unwrap();
    assert!(Arc::ptr_eq(&converted, &again));

    // At its own rate, it's the decoded file.
    let native = store.load_at_rate(hash, 44_100).unwrap();
    assert_eq!(*native, decode_file(&path).unwrap());
}

#[test]
fn notices_missing_and_changed_files() {
    // Spec: audio-import R9
    let dir = TempDir::new("store-check");
    let path = copy_fixture("sine-44k1-s16-2ch.wav", dir.path());
    let store = MediaStore::new();
    let hash = store.import(&path).unwrap();
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Available);

    // Change one sample. The length stays the same, so the store sees the
    // change by its modification time; set it explicitly, because a file
    // system's clock can be coarser than the time this test takes.
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    fs::write(&path, &bytes).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let now = ContentHash::of_bytes(&bytes);
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Changed { now });
    assert!(matches!(
        store.load(hash),
        Err(MediaError::Changed { now: n, .. }) if n == now
    ));

    fs::remove_file(&path).unwrap();
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Missing);
    assert!(matches!(store.load(hash), Err(MediaError::Missing { .. })));
    assert!(matches!(store.open(hash), Err(MediaError::Missing { .. })));

    let unknown = ContentHash::of_bytes(b"never imported");
    assert!(matches!(
        store.check(unknown),
        Err(MediaError::UnknownMedia(h)) if h == unknown
    ));
}

#[test]
fn relinks_a_moved_file_by_content() {
    // Spec: audio-import R10
    let dir = TempDir::new("store-relink");
    let path = copy_fixture("noise-44k1-s16-2ch.flac", &dir.path().join("Samples"));
    let store = MediaStore::new();
    let hash = store.import(&path).unwrap();

    // The user moves the file, and leaves a decoy of the same length with
    // other content where a name search would look first.
    let moved = dir.path().join("Elsewhere/deep/folder/renamed.flac");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&path, &moved).unwrap();
    let mut decoy = fs::read(&moved).unwrap();
    let middle = decoy.len() / 2;
    decoy[middle] ^= 0xFF;
    fs::create_dir_all(dir.path().join("Elsewhere/a")).unwrap();
    fs::write(
        dir.path().join("Elsewhere/a/noise-44k1-s16-2ch.flac"),
        &decoy,
    )
    .unwrap();
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Missing);

    let found = store.relink(hash, &[dir.path()]).unwrap();
    assert_eq!(found.as_deref(), Some(moved.as_path()));
    assert_eq!(store.entry(hash).unwrap().path, moved);
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Available);
    assert_eq!(*store.load(hash).unwrap(), decode_file(&moved).unwrap());

    // Nothing to find.
    let empty = TempDir::new("store-relink-empty");
    assert_eq!(store.relink(hash, &[empty.path()]).unwrap(), None);
}

#[test]
fn relinks_every_missing_file_in_one_pass() {
    // Spec: audio-import R10
    let dir = TempDir::new("store-relink-all");
    let store = MediaStore::new();
    let names = [
        "sine-44k1-s16-2ch.wav",
        "sine-44k1-s16-2ch.mp3",
        "sweep-48k-s24-1ch.flac",
    ];
    let mut hashes = Vec::new();
    for name in names {
        let path = copy_fixture(name, &dir.path().join("Project/Samples"));
        hashes.push(store.import(&path).unwrap());
    }
    // Two of the three move; one stays.
    let backup = dir.path().join("Backup Drive");
    fs::create_dir_all(&backup).unwrap();
    for name in &names[..2] {
        fs::rename(
            dir.path().join("Project/Samples").join(name),
            backup.join(name),
        )
        .unwrap();
    }

    let mut relinked: Vec<(ContentHash, PathBuf)> = store
        .relink_candidates(&[&backup])
        .unwrap()
        .into_iter()
        .filter_map(|(hash, paths)| Some((hash, paths.first()?.clone())))
        .collect();
    for (hash, path) in &relinked {
        store.relocate(*hash, path);
    }
    relinked.sort();
    let mut expected = vec![
        (hashes[0], backup.join(names[0])),
        (hashes[1], backup.join(names[1])),
    ];
    expected.sort();
    assert_eq!(relinked, expected);
    for hash in hashes {
        assert_eq!(store.check(hash).unwrap(), MediaStatus::Available);
    }
}

#[test]
fn references_from_a_project_are_checked_when_used() {
    let dir = TempDir::new("store-reference");
    let path = copy_fixture("sine-48k-s24-2ch.aac.m4a", dir.path());
    let hash = ContentHash::of_file(&path).unwrap();
    let size = fs::metadata(&path).unwrap().len();

    let store = MediaStore::new();
    store.add_reference(hash, &path, size);
    assert!(store.contains(hash));
    assert_eq!(store.entry(hash).unwrap().info, None);
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Available);
    assert_eq!(store.info(hash).unwrap().sample_rate, 48_000);
    assert_eq!(store.load(hash).unwrap().frames(), 12_000);

    // A reference whose file holds something else is caught before use.
    let wrong = ContentHash::of_bytes(b"something else");
    store.add_reference(wrong, &path, size);
    assert!(matches!(store.load(wrong), Err(MediaError::Changed { .. })));
}

#[test]
fn the_store_is_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>() {}
    assert_send_sync::<MediaStore>();
    assert_send::<Decoder>();

    let dir = TempDir::new("store-threads");
    let path = copy_fixture("noise-44k1-s16-2ch.wav", dir.path());
    let store = Arc::new(MediaStore::new());
    let hash = store.import(&path).unwrap();
    let loads: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || store.load_at_rate(hash, 48_000).unwrap())
        })
        .collect();
    let results: Vec<_> = loads.into_iter().map(|t| t.join().unwrap()).collect();
    for data in &results[1..] {
        assert_eq!(**data, *results[0]);
    }
}

#[test]
fn the_same_source_with_the_same_edits_is_rendered_once_and_shared() {
    // Spec: multisampler R57
    let dir = TempDir::new("store-edited");
    let path = copy_fixture("sine-44k1-s16-2ch.flac", dir.path());
    let store = MediaStore::new();
    let hash = store.import(&path).unwrap();
    let source = store.load(hash).unwrap();
    let trim = Edit {
        kind: EditKind::Trim,
        range: Some((100, 1_100)),
    };
    let first = store.load_edited(hash, &[trim]).unwrap();
    assert_eq!(first.frames(), 1_000);
    assert_eq!(
        first.channel(0).unwrap(),
        &source.channel(0).unwrap()[100..1_100]
    );
    // The same list shares the copy; a different list makes another.
    assert!(Arc::ptr_eq(
        &first,
        &store.load_edited(hash, &[trim]).unwrap()
    ));
    let reverse = Edit {
        kind: EditKind::Reverse,
        range: None,
    };
    assert!(!Arc::ptr_eq(
        &first,
        &store.load_edited(hash, &[trim, reverse]).unwrap()
    ));
    // No edits is the source itself, and the file's bytes are untouched.
    assert!(Arc::ptr_eq(&source, &store.load_edited(hash, &[]).unwrap()));
    assert_eq!(ContentHash::of_file(&path).unwrap(), hash);
    // At another rate the edits apply to the sample's own frames first.
    let at_48k = store.load_edited_at_rate(hash, 48_000, &[trim]).unwrap();
    assert_eq!(at_48k.sample_rate(), 48_000);
    assert_eq!(at_48k.frames(), converted_len(1_000, 44_100, 48_000));
}

#[test]
fn importing_a_good_copy_replaces_a_path_that_now_holds_other_content() {
    let dir = TempDir::new("store-overwritten");
    let a = copy_fixture("sine-44k1-s16-2ch.wav", dir.path());
    let backup = copy_fixture("sine-44k1-s16-2ch.wav", &dir.path().join("backup"));
    let store = MediaStore::new();
    let hash = store.import(&a).unwrap();

    // An editor overwrites the file the store knows.
    fs::copy(fixtures_dir().join("sweep-44k1-s16-2ch.wav"), &a).unwrap();
    assert!(matches!(
        store.check(hash).unwrap(),
        MediaStatus::Changed { .. }
    ));

    assert_eq!(store.import(&backup).unwrap(), hash);
    assert_eq!(store.entry(hash).unwrap().path, backup);
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Available);
}
