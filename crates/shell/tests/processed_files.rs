//! Processed audio files (WP-4.20): the one writer for the files Reverse,
//! Crop Sample, Consolidate, Freeze and Bounce make, driven directly since
//! its callers come later. The behaviour is in the audio clip editing spec,
//! R1 to R6.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_media::{BitDepth, Decoder, Dither, EncodeSpec, FileType};
use bl_model::{ClipData, TrackId, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::processed::{Kind, Request, Written};
use bl_shell::settings::Preferences;
use bl_shell::{Effect, Shell};
use serde_json::json;

bl_rt::install_trap!();

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-processed-{name}-{}-{}",
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

fn shell(config: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

fn save_as(shell: &mut Shell, path: &Path) -> PathBuf {
    PathBuf::from(
        shell
            .run("set.save_as", json!({ "path": text(path) }))
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    )
}

/// A request for `frames` stereo frames at 44.1 kHz of a ramp starting at
/// `seed`, so two seeds make different bytes.
fn request(kind: Kind, name: &str, frames: usize, seed: f32) -> Request {
    Request {
        kind,
        name: name.to_owned(),
        spec: EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: 44_100,
            channels: 2,
        },
        frames: frames as u64,
        produce: Box::new(move |sink| {
            for chunk in 0..frames.div_ceil(256) {
                let from = chunk * 256;
                let samples: Vec<f32> = (from..frames.min(from + 256))
                    .flat_map(|i| {
                        let v = seed + i as f32 / 10_000.0;
                        [v, -v]
                    })
                    .collect();
                sink.write(&samples)?;
            }
            Ok(())
        }),
    }
}

/// Writes a file and waits for it; the edit that uses it is a clip on the
/// first audio track, as a caller's would be.
fn write(shell: &mut Shell, request: Request) -> Written {
    let track = audio_track(shell);
    let landed = std::rc::Rc::new(std::cell::Cell::new(None));
    let slot = std::rc::Rc::clone(&landed);
    shell
        .start_processed(request, move |shell, written| {
            shell
                .run(
                    "browser.add_to_arrangement",
                    json!({ "paths": [text(&written.path)], "track": track, "at": 0 }),
                )
                .map_err(|e| e.to_string())?;
            slot.set(Some(written));
            Ok(())
        })
        .unwrap();
    shell.wait_processed();
    landed.take().expect("the edit landed")
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has audio tracks")
}

fn files_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// Spec: audio-clip-editing R1, R2.
#[test]
fn a_processed_file_lands_whole_in_its_kinds_folder_as_32_bit_float() {
    let scratch = Scratch::new("kinds");
    let mut shell = shell(&scratch.path("config"));
    let song = save_as(&mut shell, &scratch.path("Song.blset"));
    let project = song.parent().unwrap();
    assert!(
        !project.join("Samples/Processed").exists(),
        "a folder is made the first time it's needed"
    );
    for (seed, kind) in [
        Kind::Reverse,
        Kind::Crop,
        Kind::Consolidate,
        Kind::Freeze,
        Kind::Bounce,
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("Tone {}.wav", kind.folder());
        // Different bytes in each, or the set would keep one copy (R3).
        let written = write(&mut shell, request(kind, &name, 1_000, seed as f32));
        let folder = project.join("Samples/Processed").join(kind.folder());
        assert_eq!(written.path, folder.join(&name));
        assert_eq!(files_in(&folder), [name], "whole, with nothing hidden");
    }

    let path = project.join("Samples/Processed/Reverse/Tone Reverse.wav");
    let decoder = Decoder::open(&path).unwrap();
    let info = decoder.info();
    assert_eq!((info.sample_rate, info.channels), (44_100, 2));
    assert_eq!(info.bits_per_sample, Some(32));
    // A WAV header: the extensible format, whose sub-format 3 is IEEE
    // floating point.
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[20..22], &0xFFFEu16.to_le_bytes());
    assert_eq!(&bytes[44..46], &3u16.to_le_bytes());
    assert_eq!(info.frames, Some(1_000));
}

// Spec: audio-clip-editing R3.
#[test]
fn a_second_file_of_the_same_name_gets_2_and_nothing_is_overwritten() {
    let scratch = Scratch::new("names");
    let mut shell = shell(&scratch.path("config"));
    let song = save_as(&mut shell, &scratch.path("Song.blset"));
    let folder = song.parent().unwrap().join("Samples/Processed/Crop");
    let first = write(&mut shell, request(Kind::Crop, "Kick Crop.wav", 500, 0.1));
    let second = write(&mut shell, request(Kind::Crop, "Kick Crop.wav", 500, 0.2));
    let third = write(&mut shell, request(Kind::Crop, "Kick Crop.wav", 500, 0.3));
    assert_eq!(first.path, folder.join("Kick Crop.wav"));
    assert_eq!(second.path, folder.join("Kick Crop 2.wav"));
    assert_eq!(third.path, folder.join("Kick Crop 3.wav"));
    assert_ne!(first.hash, second.hash);
}

// Spec: audio-clip-editing R3.
#[test]
fn bytes_the_set_already_has_are_not_kept_twice() {
    let scratch = Scratch::new("same");
    let mut shell = shell(&scratch.path("config"));
    let song = save_as(&mut shell, &scratch.path("Song.blset"));
    let folder = song.parent().unwrap().join("Samples/Processed/Reverse");
    let first = write(
        &mut shell,
        request(Kind::Reverse, "A Reverse.wav", 500, 0.1),
    );
    let again = write(
        &mut shell,
        request(Kind::Reverse, "B Reverse.wav", 500, 0.1),
    );
    assert_eq!(
        again.path, first.path,
        "the clip names the media the set has"
    );
    assert_eq!(again.hash, first.hash);
    assert_eq!(files_in(&folder), ["A Reverse.wav"]);
}

// Spec: audio-clip-editing R6.
#[test]
fn a_write_that_fails_part_way_leaves_no_file_and_the_set_as_it_was() {
    let scratch = Scratch::new("fails");
    let mut shell = shell(&scratch.path("config"));
    let song = save_as(&mut shell, &scratch.path("Song.blset"));
    let folder = song.parent().unwrap().join("Samples/Processed/Crop");
    let revision = shell.document_revision();
    let failing = Request {
        produce: Box::new(|sink| {
            sink.write(&[0.5; 2_000])?;
            Err("the disk is full".to_owned())
        }),
        ..request(Kind::Crop, "Kick Crop.wav", 4_000, 0.0)
    };
    let edited = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = std::rc::Rc::clone(&edited);
    shell
        .start_processed(failing, move |_, _| {
            flag.set(true);
            Ok(())
        })
        .unwrap();
    shell.wait_processed();

    assert!(!edited.get(), "the edit never landed");
    assert!(files_in(&folder).is_empty(), "{:?}", files_in(&folder));
    assert_eq!(shell.document_revision(), revision);
    assert!(!shell.is_dirty());
    let message = shell.message().expect("a message in the Status Bar");
    assert!(message.error);
    assert!(
        message.text.contains("the disk is full"),
        "{}",
        message.text
    );
}

// Spec: audio-clip-editing R6. The progress shows in the Status Bar.
#[test]
fn the_status_bar_shows_the_progress_while_a_file_is_written() {
    let scratch = Scratch::new("progress");
    let mut shell = shell(&scratch.path("config"));
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let held = Request {
        produce: Box::new(move |sink| {
            sink.write(&[0.5; 1_000])?;
            wait.recv().map_err(|e| e.to_string())?;
            sink.write(&[0.5; 1_000])
        }),
        ..request(Kind::Bounce, "Held Bounce.wav", 1_000, 0.0)
    };
    shell.start_processed(held, |_, _| Ok(())).unwrap();
    assert!(shell.is_processing());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        shell.poll();
        let text = shell.message().map(|m| m.text.clone()).unwrap_or_default();
        // Half of the stereo frames are written; the rest waits.
        if text == "Writing Held Bounce.wav (50%)" {
            break;
        }
        assert!(text.starts_with("Writing Held Bounce.wav"), "{text}");
        assert!(Instant::now() < deadline, "{text}");
        std::thread::sleep(Duration::from_millis(2));
    }
    go.send(()).unwrap();
    shell.wait_processed();
    assert!(shell.message().is_none(), "the message goes when it lands");
}

// Spec: audio-clip-editing R4, R5.
#[test]
fn an_unsaved_sets_files_move_into_the_project_on_its_first_save() {
    let scratch = Scratch::new("temporary");
    let mut shell = shell(&scratch.path("config"));
    let temporary = scratch.path("Temp");
    shell
        .run(
            "settings.set_temporary_folder",
            json!({ "path": text(&temporary) }),
        )
        .unwrap();
    let written = write(
        &mut shell,
        request(Kind::Reverse, "Pad Reverse.wav", 800, 0.1),
    );
    assert_eq!(written.path, temporary.join("Reverse/Pad Reverse.wav"));
    assert!(written.path.is_file());
    // The project already has a file of that name, so R3's rule applies on
    // the move.
    let mut other = self::shell(&scratch.path("config3"));
    let first = save_as(&mut other, &scratch.path("Song.blset"));
    write(
        &mut other,
        request(Kind::Reverse, "Pad Reverse.wav", 800, 0.9),
    );
    let project = first.parent().unwrap().to_owned();
    let taken = project.join("Samples/Processed/Reverse");

    let song = save_as(&mut shell, &project.join("Second.blset"));
    assert_eq!(song.parent().unwrap(), project);
    let moved = taken.join("Pad Reverse 2.wav");
    assert!(
        moved.is_file(),
        "moved into the project: {:?}",
        files_in(&taken)
    );
    assert!(!written.path.exists(), "and out of the Temporary Folder");
    assert_eq!(
        bl_media::ContentHash::of_file(&moved).unwrap(),
        written.hash,
        "byte for byte"
    );

    // The media table points at the new path, relative to the project.
    let (saved, _) = bl_project::load(&song).unwrap();
    let paths: Vec<&str> = saved.media.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["Samples/Processed/Reverse/Pad Reverse 2.wav"]);

    // The shell's media store follows the file, and the reopened set finds
    // it where the table says.
    let entry = shell.media_store().entry(written.hash).unwrap();
    assert_eq!(entry.path, moved);
    let mut reopened = self::shell(&scratch.path("config2"));
    reopened
        .run("set.open", json!({ "path": text(&song) }))
        .unwrap();
    let hash = {
        let document = reopened.editor().document();
        let clip = document
            .tracks()
            .flat_map(|t| t.clips())
            .next()
            .expect("the clip came with the set");
        match &clip.data {
            ClipData::Audio(audio) => bl_media::ContentHash::from_bytes(audio.media.0),
            ClipData::Midi(_) => unreachable!(),
        }
    };
    assert_eq!(hash, written.hash);
    assert_eq!(reopened.media_store().entry(hash).unwrap().path, moved);
}

// Spec: audio-clip-editing R4. Undo leaves the file; redo uses it again.
#[test]
fn undo_leaves_the_file_on_disk_and_redo_uses_it_again() {
    let scratch = Scratch::new("undo");
    let mut shell = shell(&scratch.path("config"));
    save_as(&mut shell, &scratch.path("Song.blset"));
    let written = write(&mut shell, request(Kind::Crop, "Hit Crop.wav", 600, 0.1));
    let clips = |shell: &Shell| {
        shell
            .editor()
            .document()
            .tracks()
            .map(|t| t.clips().count())
            .sum::<usize>()
    };
    assert_eq!(clips(&shell), 1);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(clips(&shell), 0, "one undo step took the edit back");
    assert!(written.path.is_file(), "the file stays");
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(clips(&shell), 1);
    let (saved, _) = {
        let set = shell.run("set.save", json!({})).unwrap();
        bl_project::load(Path::new(set["path"].as_str().unwrap())).unwrap()
    };
    assert_eq!(saved.media.len(), 1, "one file, used again");
    assert_eq!(files_in(written.path.parent().unwrap()), ["Hit Crop.wav"]);
}

// Spec: audio-clip-editing R5. The Temporary Folder is a kept preference.
#[test]
fn the_temporary_folder_is_a_kept_setting_that_reads_back() {
    let scratch = Scratch::new("setting");
    let config = scratch.path("config");
    let mut shell = shell(&config);
    let state = |shell: &mut Shell| {
        shell.run("settings.state", json!({})).unwrap()["file_folder"]["temporary_folder"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        state(&mut shell),
        text(&config.join("Temporary")),
        "the default is in the application-data folder"
    );
    let chosen = scratch.path("Scratch Space");
    let result = shell
        .run(
            "settings.set_temporary_folder",
            json!({ "path": text(&chosen) }),
        )
        .unwrap();
    assert_eq!(result["path"], text(&chosen));
    assert_eq!(state(&mut shell), text(&chosen));

    let prefs = Preferences::load(&config).unwrap();
    let relaunched = Shell::new(
        prefs,
        Some(config.clone()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    assert_eq!(relaunched.temporary_folder(), chosen, "kept between starts");

    shell
        .run("settings.set_temporary_folder", json!({ "path": null }))
        .unwrap();
    assert_eq!(state(&mut shell), text(&config.join("Temporary")));
    assert!(
        shell
            .run(
                "settings.set_temporary_folder",
                json!({ "path": "relative" })
            )
            .is_err(),
        "a path must be absolute"
    );
}

// The Browse button: a person's invocation without a path asks for a folder.
#[test]
fn browse_asks_for_a_folder() {
    let mut shell = Shell::headless();
    shell
        .invoke("settings.set_temporary_folder", json!({}))
        .unwrap();
    assert_eq!(shell.take_effects(), [Effect::PickTemporaryFolder]);
}

// Spec: audio-clip-editing R6. The audio thread plays on, off the heap,
// while a file is written.
#[test]
fn the_engine_plays_on_off_the_heap_while_a_file_is_written() {
    assert!(bl_rt::trap::is_installed());
    let scratch = Scratch::new("realtime");
    let mut shell = shell(&scratch.path("config"));
    shell.start_audio();
    let blocks = |shell: &Shell| shell.audio().stats().expect("a stream is open").blocks;
    let started = blocks(&shell);
    // Two seconds of audio, written in small steps so the write overlaps
    // many blocks.
    let slow = Request {
        produce: Box::new(|sink| {
            for _ in 0..100 {
                sink.write(&vec![0.25; 2 * 1_024])?;
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        }),
        ..request(Kind::Freeze, "Long Freeze.wav", 102_400, 0.0)
    };
    shell.start_processed(slow, |_, _| Ok(())).unwrap();
    let during = std::cell::Cell::new(0);
    while shell.is_processing() {
        shell.poll();
        during.set(blocks(&shell));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        during.get() > started + 4,
        "the engine ran blocks while the file was written: {started} to {}",
        during.get()
    );
    let stats = shell.audio().stats().unwrap();
    assert_eq!(stats.heap_operations, 0, "{stats:?}");
    shell.audio_mut().stop();
}

// Spec: audio-clip-editing R6. A file still being written for a set that
// is replaced is deleted, and its edit never lands in the new set.
#[test]
fn a_new_set_drops_a_file_still_being_written() {
    let scratch = Scratch::new("replaced");
    let mut shell = shell(&scratch.path("config"));
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let held = Request {
        produce: Box::new(move |sink| {
            sink.write(&[0.5; 1_000])?;
            wait.recv().map_err(|e| e.to_string())?;
            sink.write(&[0.5; 1_000])
        }),
        ..request(Kind::Freeze, "Held Freeze.wav", 1_000, 0.0)
    };
    let landed = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = std::rc::Rc::clone(&landed);
    shell
        .start_processed(held, move |_, _| {
            flag.set(true);
            Ok(())
        })
        .unwrap();
    shell.run("set.new", json!({})).unwrap();
    assert!(!shell.is_processing());
    go.send(()).unwrap();
    let folder = shell.temporary_folder().join("Freeze");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !files_in(&folder).is_empty() {
        assert!(Instant::now() < deadline, "{:?}", files_in(&folder));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!landed.get());
}
