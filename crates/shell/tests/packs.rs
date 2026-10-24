//! Packing a project into one file and unpacking it (WP-6.35b; the set
//! exchange spec, R16 to R24). Driven through `Shell::run`, as an agent drives
//! it (D17), on real folders and bl-project's real set files.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use bl_model::TrackKind;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Effect, Shell};
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-packs-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Writes a short 16-bit mono WAV file that isn't silence.
fn wav(path: &Path, frames: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let phase = f64::from(i) * 440.0 / 48_000.0 * std::f64::consts::TAU;
        let sample = (phase.sin() * 32.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn headless(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// Puts the files at `paths` on the first audio track, then saves the set.
fn add_files(shell: &mut Shell, paths: &[&Path]) {
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has audio tracks");
    for (index, path) in paths.iter().enumerate() {
        shell
            .run(
                "browser.add_to_arrangement",
                json!({ "paths": [text(path)], "track": track, "at": index as u64 * 100_000 }),
            )
            .unwrap();
    }
    shell.run("set.save", json!({})).unwrap();
}

/// A project of two sets that share a folder of samples, with a subfolder, an
/// empty folder, and what a pack leaves out: `Backup` (the second save of
/// `Song` makes it) and a journal. Returns the project folder.
fn project(scratch: &Scratch) -> PathBuf {
    let mut shell = headless(scratch);
    let song = shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Song.blset")) }),
        )
        .unwrap()["path"]
        .as_str()
        .map(PathBuf::from)
        .unwrap();
    let folder = song.parent().unwrap().to_path_buf();
    let kick = folder.join("Samples/Imported/Kick.wav");
    wav(&kick, 4800);
    add_files(&mut shell, &[&kick]);
    shell.run("set.save", json!({})).unwrap();

    shell.run("set.new", json!({})).unwrap();
    shell
        .run(
            "set.save_as",
            json!({ "path": text(&folder.join("Other.blset")) }),
        )
        .unwrap();
    let snare = folder.join("Samples/Recorded/Snare.wav");
    wav(&snare, 2400);
    add_files(&mut shell, &[&snare, &kick]);

    std::fs::create_dir_all(folder.join("Extras/Empty")).unwrap();
    std::fs::write(folder.join("Extras/readme.txt"), b"hello").unwrap();
    std::fs::write(folder.join(".Song.blset.journal"), b"journal").unwrap();
    assert!(
        folder.join("Backup").is_dir(),
        "a second save made a backup"
    );
    folder
}

/// Every folder and file under `root` that a pack holds, by path, with the
/// file's bytes.
fn packed_tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(root: &Path, rel: &Path, into: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(root.join(rel)).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || (rel.as_os_str().is_empty() && name == "Backup") {
                continue;
            }
            let path = rel.join(&name);
            let key = path.to_string_lossy().replace('\\', "/");
            if entry.file_type().unwrap().is_dir() {
                into.insert(key, None);
                walk(root, &path, into);
            } else {
                into.insert(key, Some(std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut into = BTreeMap::new();
    walk(root, Path::new(""), &mut into);
    into
}

/// Whether anything but `expected` is in `folder`.
fn holds_only(folder: &Path, expected: &[&str]) -> bool {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names == expected
}

// Spec: set-exchange R16, R17, R19, R20, R22, R24.
#[test]
fn a_project_of_two_sets_packs_and_unpacks_elsewhere_and_its_sets_open_with_every_sample() {
    let scratch = Scratch::new("round-trip");
    let folder = project(&scratch);
    let pack = scratch.path("Song Project.blpack");

    // The open set's project is the default (R24).
    let mut shell = headless(&scratch);
    shell
        .run(
            "set.open",
            json!({ "path": text(&folder.join("Other.blset")) }),
        )
        .unwrap();
    let packed = shell
        .run("set.create_pack", json!({ "path": text(&pack) }))
        .unwrap();
    assert_eq!(packed["path"], text(&pack));
    assert_eq!(packed["files"], 5, "two sets, two samples and the readme");
    assert_eq!(packed["left_out"], json!([]));
    assert!(
        shell
            .message()
            .is_some_and(|m| m.text.contains("Song Project.blpack") && !m.error),
        "the Status Bar says where the pack is (R16)"
    );

    let out = scratch.path("elsewhere");
    std::fs::create_dir_all(&out).unwrap();
    let installed = shell
        .run(
            "set.install_pack",
            json!({ "pack": text(&pack), "folder": text(&out) }),
        )
        .unwrap();
    let unpacked = out.join("Song Project");
    assert_eq!(installed["folder"], text(&unpacked));
    assert_eq!(installed["sets"].as_array().unwrap().len(), 2);
    assert!(
        shell
            .message()
            .is_some_and(|m| m.text.contains("Other.blset"))
    );
    // Every file is byte for byte, and the pack left out what it should.
    assert_eq!(packed_tree(&unpacked), packed_tree(&folder));
    assert!(!unpacked.join("Backup").exists());
    assert!(!unpacked.join(".Song.blset.journal").exists());
    assert!(unpacked.join("Extras/Empty").is_dir());
    // Unpacking changed nothing in the open set (R20).
    assert_eq!(shell.set_path(), Some(folder.join("Other.blset").as_path()));

    // With the first project out of the way, each set finds its samples.
    std::fs::rename(&folder, scratch.path("Moved")).unwrap();
    for set in ["Song.blset", "Other.blset"] {
        let mut shell = headless(&scratch);
        shell
            .run("set.open", json!({ "path": text(&unpacked.join(set)) }))
            .unwrap();
        assert_eq!(shell.missing_files(), 0, "{set} lost a sample");
    }
}

// Spec: set-exchange R17, R24.
#[test]
fn a_sample_from_outside_the_project_is_named_in_the_result() {
    let scratch = Scratch::new("external");
    let mut shell = headless(&scratch);
    let song = shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Song.blset")) }),
        )
        .unwrap()["path"]
        .as_str()
        .map(PathBuf::from)
        .unwrap();
    let inside = song.parent().unwrap().join("Samples/Imported/Kick.wav");
    let outside = scratch.path("elsewhere/Tone.wav");
    wav(&inside, 4800);
    wav(&outside, 2400);
    add_files(&mut shell, &[&inside, &outside]);

    let packed = shell
        .run(
            "set.create_pack",
            json!({ "path": text(&scratch.path("Song.blpack")) }),
        )
        .unwrap();
    assert_eq!(packed["left_out"], json!([text(&outside)]));
    assert!(
        shell
            .message()
            .is_some_and(|m| m.text.contains("1 file used from outside"))
    );
}

// Spec: set-exchange R18, R24.
#[test]
fn packing_the_open_projects_unsaved_changes_or_a_set_never_saved_is_refused() {
    let scratch = Scratch::new("unsaved");
    let folder = project(&scratch);
    let pack = scratch.path("p.blpack");
    let mut shell = headless(&scratch);

    // Never saved: no project to pack.
    let error = shell
        .run("set.create_pack", json!({ "path": text(&pack) }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("save it first"), "{error}");

    shell
        .run(
            "set.open",
            json!({ "path": text(&folder.join("Song.blset")) }),
        )
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": "New" }))
        .unwrap();
    let error = shell
        .run("set.create_pack", json!({ "path": text(&pack) }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsaved changes"), "{error}");
    assert!(!pack.exists());

    // The refusal is the open project's: another project folder packs.
    let other = scratch.path("Other Project");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Other.blset"), b"set").unwrap();
    shell
        .run(
            "set.create_pack",
            json!({ "project": text(&other), "path": text(&pack) }),
        )
        .unwrap();
    assert!(pack.is_file());
    std::fs::remove_file(&pack).unwrap();

    shell.run("set.save", json!({})).unwrap();
    shell
        .run("set.create_pack", json!({ "path": text(&pack) }))
        .unwrap();
    assert!(pack.is_file());
}

/// A pack with a good manifest and these entries, as an agent might be given.
fn pack_with(schema: u32, entries: &[(&str, &[u8])]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("beatlayer-pack.json", options).unwrap();
    let manifest = json!({
        "format": "beatlayer-pack", "schema": schema, "written_by": "0.0.0",
        "project": "Song Project", "sets": [],
    });
    zip.write_all(manifest.to_string().as_bytes()).unwrap();
    for (name, bytes) in entries {
        if let Some(name) = name.strip_prefix("link:") {
            zip.add_symlink(name, String::from_utf8_lossy(bytes), options)
                .unwrap();
        } else {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}

// Spec: set-exchange R21, R24.
#[test]
fn packs_the_reader_refuses_write_nothing() {
    let scratch = Scratch::new("refused");
    let mut shell = headless(&scratch);
    let out = scratch.path("out");
    std::fs::create_dir_all(&out).unwrap();
    let good = b"Song.blset body";
    let cases: [(&str, Vec<u8>, &str); 5] = [
        (
            "absolute",
            pack_with(1, &[("/tmp/Song.blset", good)]),
            "absolute",
        ),
        (
            "climbing",
            pack_with(1, &[("Song Project/../Song.blset", good)]),
            "climbs out",
        ),
        (
            "link",
            pack_with(1, &[("link:Song Project/Song.blset", b"/etc/passwd")]),
            "is a link",
        ),
        (
            "newer",
            pack_with(2, &[("Song Project/Song.blset", good)]),
            "later version",
        ),
        (
            "checksum",
            {
                let mut bytes = pack_with(1, &[("Song Project/Song.blset", good)]);
                let at = bytes.windows(good.len()).position(|w| w == good).unwrap();
                bytes[at] ^= 0xFF;
                bytes
            },
            "damaged",
        ),
    ];
    for (label, bytes, about) in cases {
        let pack = scratch.path(&format!("{label}.blpack"));
        std::fs::write(&pack, bytes).unwrap();
        let error = shell
            .run(
                "set.install_pack",
                json!({ "pack": text(&pack), "folder": text(&out) }),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains(about), "{label}: {error}");
        assert!(
            holds_only(&out, &[]),
            "{label} left something in the folder"
        );
        assert!(shell.message().is_some_and(|m| m.error), "{label}");
    }
}

// Spec: set-exchange R20.
#[test]
fn a_pack_installed_where_its_folder_is_taken_makes_the_next_numbered_name() {
    let scratch = Scratch::new("numbered");
    let folder = project(&scratch);
    let pack = scratch.path("p.blpack");
    let mut shell = headless(&scratch);
    shell
        .run(
            "set.create_pack",
            json!({ "project": text(&folder), "path": text(&pack) }),
        )
        .unwrap();
    let out = scratch.path("out");
    std::fs::create_dir_all(&out).unwrap();
    let args = json!({ "pack": text(&pack), "folder": text(&out) });
    for expected in ["Song Project", "Song Project 2", "Song Project 3"] {
        let installed = shell.run("set.install_pack", args.clone()).unwrap();
        assert_eq!(installed["folder"], text(&out.join(expected)));
    }
    assert!(holds_only(
        &out,
        &["Song Project", "Song Project 2", "Song Project 3"]
    ));
}

// Spec: set-exchange R20, R23.
#[test]
fn a_person_is_asked_where_and_the_work_runs_on_a_thread_that_can_be_cancelled() {
    let scratch = Scratch::new("person");
    let folder = project(&scratch);
    let mut shell = headless(&scratch);
    shell
        .run(
            "set.open",
            json!({ "path": text(&folder.join("Song.blset")) }),
        )
        .unwrap();

    // Create Pack asks where to save, the project's parent first.
    shell.invoke("set.create_pack", json!({})).unwrap();
    assert_eq!(
        shell.take_effects(),
        [Effect::PickPackPath {
            project: None,
            file_name: "Song Project.blpack".to_owned(),
            folder: folder.parent().unwrap().to_path_buf(),
        }]
    );
    // Install Pack... asks for the pack, then for the folder.
    shell.invoke("set.install_pack", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::PickPack]);
    shell
        .invoke("set.install_pack", json!({ "pack": "/some/Song.blpack" }))
        .unwrap();
    assert_eq!(
        shell.take_effects(),
        [Effect::PickInstallFolder {
            pack: "/some/Song.blpack".to_owned()
        }]
    );

    // With the path, the work starts and the call returns at once.
    let pack = scratch.path("p.blpack");
    assert_eq!(shell.pack_status().job, None);
    let started = shell
        .invoke("set.create_pack", json!({ "path": text(&pack) }))
        .unwrap();
    assert_eq!(started, Value::Null);
    let status = shell.run("set.pack_status", json!({})).unwrap();
    assert_eq!(status["job"], "create");
    let error = shell
        .run(
            "set.create_pack",
            json!({ "path": text(&scratch.path("q.blpack")) }),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("under way"), "{error}");

    // Cancelled before it ends, it leaves no pack; if it ended first, it
    // left a whole one. Never a half.
    shell.run("set.cancel_pack", json!({})).unwrap();
    let ended = shell.wait_pack().expect("the work was started");
    assert_eq!(ended.is_ok(), pack.is_file());
    let hidden = std::fs::read_dir(scratch.path(""))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        })
        .count();
    assert_eq!(hidden, 0);
    assert_eq!(shell.pack_status().job, None);
}
