//! Packs (the set exchange spec, R17 and R19 to R23): a project folder in one
//! file and out again, and the packs the reader refuses.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use bl_project::pack::{self, Error, FORMAT, Manifest, SCHEMA};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::common::TestDir;

static NEVER: AtomicBool = AtomicBool::new(false);

/// A project folder of two sets, samples in two folders, an empty folder,
/// and what a pack leaves out: `Backup`, a journal and a save under way.
fn project(dir: &Path) -> PathBuf {
    let root = dir.join("Song Project");
    for folder in ["Samples/Imported", "Samples/Recorded", "Empty", "Backup"] {
        fs::create_dir_all(root.join(folder)).unwrap();
    }
    let mut state = 0x2545_F491u32;
    let noise: Vec<u8> = (0..300_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 7) as u8
        })
        .collect();
    for (path, bytes) in [
        ("Song.blset", b"first set".to_vec()),
        ("Other.blset", b"second set".to_vec()),
        ("Samples/Imported/tone.wav", vec![7u8; 700_000]),
        ("Samples/Recorded/noise.bin", noise),
        ("notes.txt", b"hello".to_vec()),
        ("Backup/Song [old].blset", b"older".to_vec()),
        (".Song.blset.journal", b"journal".to_vec()),
        (".Song.blset.1-0.saving", b"half a save".to_vec()),
    ] {
        fs::write(root.join(path), bytes).unwrap();
    }
    root
}

/// Every folder and file under `root`, by path, with the file's bytes.
fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(root: &Path, rel: &Path, into: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in fs::read_dir(root.join(rel)).unwrap() {
            let entry = entry.unwrap();
            let path = rel.join(entry.file_name());
            let name = path.to_string_lossy().replace('\\', "/");
            if entry.file_type().unwrap().is_dir() {
                into.insert(name, None);
                walk(root, &path, into);
            } else {
                into.insert(name, Some(fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut into = BTreeMap::new();
    walk(root, Path::new(""), &mut into);
    into
}

/// A pack with a good manifest for `project` and these entries; a name
/// ending in `/` is a folder, and a file named `link:` before its name is a
/// link to its bytes.
fn pack_of(project: &str, entries: &[(&str, &[u8])]) -> Vec<u8> {
    let manifest = Manifest {
        format: FORMAT.to_owned(),
        schema: SCHEMA,
        written_by: "0.0.0".to_owned(),
        project: project.to_owned(),
        sets: Vec::new(),
    };
    pack_with(&manifest, entries)
}

fn pack_with(manifest: &Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("beatlayer-pack.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    for (name, bytes) in entries {
        if let Some(name) = name.strip_prefix("link:") {
            zip.add_symlink(name, String::from_utf8_lossy(bytes), options)
                .unwrap();
        } else if name.ends_with('/') {
            zip.add_directory(*name, options).unwrap();
        } else {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}

/// Installs the pack made of `bytes` into an empty folder and returns the
/// outcome and what the folder holds afterwards.
fn install_bytes(dir: &TestDir, bytes: &[u8]) -> (Result<pack::Unpacked, Error>, Vec<String>) {
    let file = dir.path().join("in.blpack");
    fs::write(&file, bytes).unwrap();
    let parent = dir.path().join("out");
    fs::create_dir_all(&parent).unwrap();
    let result = pack::install(&file, &parent, &NEVER, &mut |_| {});
    let left = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    (result, left)
}

fn refused(result: Result<pack::Unpacked, Error>, left: &[String], about: &str) {
    match result {
        Err(Error::Refused(reason)) => {
            assert!(
                reason.contains(about),
                "{reason:?} should mention {about:?}"
            );
        }
        other => panic!("expected a refusal about {about:?}, got {other:?}"),
    }
    assert!(left.is_empty(), "a refused pack left {left:?}");
}

// Spec: set-exchange R17, R19, R20, R22.
#[test]
fn a_project_packs_and_unpacks_with_every_file_byte_identical() {
    let dir = TestDir::new("pack-round-trip");
    let root = project(dir.path());
    let pack_file = dir.path().join("Song Project.blpack");
    let packed = pack::create(&root, &pack_file, &NEVER, &mut |_| {}).unwrap();
    assert_eq!(packed.files, 5);
    assert_eq!(packed.bytes, 9 + 10 + 700_000 + 300_000 + 5);
    // Nothing hidden is left beside the pack.
    let beside: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        beside.iter().all(|name| !name.contains("packing")),
        "{beside:?}"
    );
    // Deflate shrinks the tone, and the noise is stored whole.
    let size = fs::metadata(&pack_file).unwrap().len();
    assert!((300_000..700_000).contains(&size), "{size}");

    let out = dir.path().join("elsewhere");
    fs::create_dir(&out).unwrap();
    let unpacked = pack::install(&pack_file, &out, &NEVER, &mut |_| {}).unwrap();
    assert_eq!(unpacked.folder, out.join("Song Project"));
    assert_eq!(unpacked.sets.len(), 2);

    let mut expected = tree(&root);
    expected.retain(|name, _| {
        !name.starts_with("Backup") && !name.split('/').any(|part| part.starts_with('.'))
    });
    assert!(expected.contains_key("Empty"));
    assert_eq!(tree(&unpacked.folder), expected);

    // A second unpack finds the name taken (R20).
    let again = pack::install(&pack_file, &out, &NEVER, &mut |_| {}).unwrap();
    assert_eq!(again.folder, out.join("Song Project 2"));
    let third = pack::install(&pack_file, &out, &NEVER, &mut |_| {}).unwrap();
    assert_eq!(third.folder, out.join("Song Project 3"));
}

// Spec: set-exchange R19.
#[test]
fn a_pack_starts_with_its_manifest_and_holds_zip64_records() {
    let dir = TestDir::new("pack-zip64");
    let root = project(dir.path());
    let pack_file = dir.path().join("p.blpack");
    pack::create(&root, &pack_file, &NEVER, &mut |_| {}).unwrap();
    let bytes = fs::read(&pack_file).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let manifest: Manifest = {
        let entry = archive.by_index(0).unwrap();
        assert_eq!(entry.name().unwrap(), "beatlayer-pack.json");
        serde_json::from_reader(entry).unwrap()
    };
    assert_eq!(manifest.format, "beatlayer-pack");
    assert_eq!(manifest.schema, SCHEMA);
    assert_eq!(manifest.project, "Song Project");
    assert_eq!(manifest.sets, ["Other.blset", "Song.blset"]);

    // Every central directory record keeps its sizes in a ZIP64 field (id
    // 0x0001), so a file over 4 GB keeps its size without one in the test.
    let mut records = 0;
    let mut at = 0;
    while let Some(found) = bytes[at..].windows(4).position(|w| w == b"PK\x01\x02") {
        let start = at + found;
        let field = |offset: usize, len: usize| &bytes[start + offset..start + offset + len];
        let name_len = u16::from_le_bytes(field(28, 2).try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(field(30, 2).try_into().unwrap()) as usize;
        assert_eq!(
            field(20, 4),
            [0xFF; 4],
            "compressed size is in the ZIP64 field"
        );
        assert_eq!(field(24, 4), [0xFF; 4], "size is in the ZIP64 field");
        let extra = field(46 + name_len, extra_len);
        assert_eq!(&extra[..2], [1, 0], "the first extra field is ZIP64's");
        records += 1;
        at = start + 4;
    }
    assert_eq!(records, archive.len());
    // The reader gives the sizes back from those fields.
    let tone = archive
        .file_names()
        .position(|name| name.unwrap().ends_with("tone.wav"))
        .unwrap();
    assert_eq!(archive.by_index(tone).unwrap().size(), 700_000);
}

// Spec: set-exchange R21.
#[test]
fn a_pack_with_a_bad_name_a_link_or_no_manifest_is_refused_with_nothing_written() {
    let cases: [(&str, Vec<u8>, &str); 7] = [
        (
            "absolute",
            pack_of("Song Project", &[("/etc/passwd", b"x")]),
            "absolute",
        ),
        (
            "climbing",
            pack_of("Song Project", &[("Song Project/../x", b"x")]),
            "climbs out",
        ),
        (
            "outside",
            pack_of("Song Project", &[("Other/x", b"x")]),
            "isn't inside",
        ),
        (
            "link",
            pack_of("Song Project", &[("link:Song Project/x", b"/etc")]),
            "is a link",
        ),
        (
            "name",
            pack_of("Song Project", &[("Song Project/a:b", b"x")]),
            "can't be a file name",
        ),
        (
            "device",
            pack_of("Song Project", &[("Song Project/NUL.txt", b"x")]),
            "can't be a file name",
        ),
        (
            "manifest",
            {
                let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
                zip.start_file("Song Project/x", SimpleFileOptions::default())
                    .unwrap();
                zip.finish().unwrap().into_inner()
            },
            "first entry isn't the manifest",
        ),
    ];
    for (label, bytes, about) in cases {
        let dir = TestDir::new(&format!("pack-refused-{label}"));
        let (result, left) = install_bytes(&dir, &bytes);
        refused(result, &left, about);
    }
}

// Spec: set-exchange R21.
#[test]
fn a_pack_from_a_later_version_or_not_a_pack_is_refused() {
    let dir = TestDir::new("pack-newer");
    let manifest = Manifest {
        format: FORMAT.to_owned(),
        schema: SCHEMA + 1,
        written_by: "99.0.0".to_owned(),
        project: "Song Project".to_owned(),
        sets: Vec::new(),
    };
    let (result, left) = install_bytes(&dir, &pack_with(&manifest, &[("Song Project/x", b"x")]));
    refused(result, &left, "99.0.0");

    let dir = TestDir::new("pack-text");
    let (result, left) = install_bytes(&dir, b"this is not a zip file");
    refused(result, &left, "can't be read");
}

// Spec: set-exchange R21.
#[test]
fn a_file_with_a_bad_checksum_or_more_bytes_than_declared_is_refused_with_nothing_written() {
    let payload = b"a distinctive run of sixteen+ bytes";
    let good = pack_of(
        "Song Project",
        &[
            ("Song Project/a.bin", payload),
            ("Song Project/b.bin", b"b"),
        ],
    );

    // A byte of a stored file changed.
    let mut damaged = good.clone();
    let at = damaged
        .windows(payload.len())
        .position(|w| w == payload)
        .unwrap();
    damaged[at + 3] ^= 0xFF;
    let dir = TestDir::new("pack-checksum");
    let (result, left) = install_bytes(&dir, &damaged);
    refused(result, &left, "damaged");

    // The central directory says the file is smaller than its bytes.
    let mut lying = good;
    let name = b"Song Project/a.bin";
    let central = lying
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"PK\x01\x02")
        .map(|(at, _)| at)
        .find(|at| &lying[at + 46..at + 46 + name.len()] == name)
        .unwrap();
    let declared = (payload.len() as u32 - 4).to_le_bytes();
    lying[central + 24..central + 28].copy_from_slice(&declared);
    let dir = TestDir::new("pack-size");
    let (result, left) = install_bytes(&dir, &lying);
    assert!(result.is_err(), "a file larger than declared unpacked");
    assert!(left.is_empty(), "{left:?}");
}

// Spec: set-exchange R19, R23.
#[test]
fn a_cancelled_pack_and_a_cancelled_unpack_leave_nothing() {
    let dir = TestDir::new("pack-cancel");
    let root = project(dir.path());
    let pack_file = dir.path().join("p.blpack");

    // Cancelled at the start, and a third of the way through.
    for stop_at in [0.0, 0.3] {
        let cancel = AtomicBool::new(stop_at == 0.0);
        let result = pack::create(&root, &pack_file, &cancel, &mut |share| {
            if share > stop_at {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        let beside: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(beside.len(), 1, "{beside:?}");
    }

    pack::create(&root, &pack_file, &NEVER, &mut |_| {}).unwrap();
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let cancel = AtomicBool::new(false);
    let result = pack::install(&pack_file, &out, &cancel, &mut |share| {
        if share > 0.3 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
}

// Spec: set-exchange R23.
#[test]
fn the_reader_checks_a_whole_pack_without_writing_and_refuses_one_that_is_too_big() {
    let dir = TestDir::new("pack-verify");
    let root = project(dir.path());
    let pack_file = dir.path().join("p.blpack");
    pack::create(&root, &pack_file, &NEVER, &mut |_| {}).unwrap();
    let bytes = fs::read(&pack_file).unwrap();
    let plan = pack::verify(Cursor::new(&bytes), 8 << 20).unwrap();
    assert_eq!(plan.manifest.project, "Song Project");
    assert!(pack::verify(Cursor::new(&bytes), 1000).is_err());
}

// Spec: set-exchange R21.
#[test]
fn a_pack_with_two_entries_differing_only_in_case_is_refused_with_nothing_written() {
    let dir = TestDir::new("pack-case-twice");
    let bytes = pack_of("P", &[("P/a.wav", b"first"), ("P/A.wav", b"second")]);
    let (result, left) = install_bytes(&dir, &bytes);
    refused(result, &left, "twice");
    assert!(matches!(
        pack::verify(Cursor::new(&bytes), u64::MAX),
        Err(Error::Refused(_))
    ));
}
